#!/usr/bin/env python3
"""Bastion pack CLI: makes any agent act as Bastion, with no daemon.

Everything lives under ``$BASTION_HOME`` (default ``~/.bastion``):

- ``memory.db``   memories (SQLite + FTS5), knowledge graph, reminders,
                  skill outcomes, persona weights and the companion.
- ``personas/``   one ``<slug>/SOUL.md`` per persona (Bastion's contract).
- ``skills/``     skills the agent wrote itself, in the agentskills format.

Standard library only. Every command prints plain text unless ``--json``.
"""

from __future__ import annotations

import argparse
import json
import os
import re
import shutil
import sqlite3
import subprocess
import sys
import time
import unicodedata
import uuid
from datetime import datetime, timedelta, timezone
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from knowledge_graph import KnowledgeGraph  # noqa: E402

VERSION = "0.1.0"
TIERS = ("cloud-ok", "local-only")
KINDS = ("fact", "preference", "decision", "event", "note", "lesson")
HOOK_BUDGET = 1500  # chars a hook may add to the host agent's context
TURN_MAX = 4000  # chars kept per conversation turn
HARNESS_TAGS = ("<command-", "<local-command", "<system-reminder", "<bash-",
                "<task-notification", "<user-prompt-submit-hook")


# ---------------------------------------------------------------- home & db


def home() -> Path:
    raw = os.environ.get("BASTION_HOME") or str(Path.home() / ".bastion")
    path = Path(raw).expanduser()
    path.mkdir(parents=True, exist_ok=True)
    return path


def now() -> datetime:
    return datetime.now(tz=timezone.utc)


def iso(dt: datetime) -> str:
    return dt.astimezone(timezone.utc).isoformat(timespec="seconds")


def parse_iso(value: str) -> datetime:
    dt = datetime.fromisoformat(value)
    if dt.tzinfo is None:
        dt = dt.astimezone()  # naive input means the person's local time
    return dt.astimezone(timezone.utc)


SCHEMA = """
CREATE TABLE IF NOT EXISTS memories (
    id          TEXT PRIMARY KEY,
    content     TEXT NOT NULL,
    kind        TEXT NOT NULL DEFAULT 'note',
    persona     TEXT,
    tags        TEXT NOT NULL DEFAULT '',
    importance  INTEGER NOT NULL DEFAULT 3,
    private     INTEGER NOT NULL DEFAULT 0,
    created_at  TEXT NOT NULL,
    last_used   TEXT,
    uses        INTEGER NOT NULL DEFAULT 0,
    valid_to    TEXT
);
CREATE INDEX IF NOT EXISTS idx_memories_persona ON memories(persona);
CREATE TABLE IF NOT EXISTS reminders (
    id          TEXT PRIMARY KEY,
    text        TEXT NOT NULL,
    due_at      TEXT NOT NULL,
    every       TEXT,
    persona     TEXT,
    created_at  TEXT NOT NULL,
    done_at     TEXT,
    shown_at    TEXT
);
CREATE TABLE IF NOT EXISTS skill_runs (
    skill       TEXT NOT NULL,
    outcome     TEXT NOT NULL,
    note        TEXT NOT NULL DEFAULT '',
    at          TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS weights (
    persona     TEXT NOT NULL,
    weight      REAL NOT NULL,
    why         TEXT NOT NULL,
    at          TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS turns (
    id          TEXT PRIMARY KEY,
    session     TEXT NOT NULL,
    seq         INTEGER NOT NULL,
    role        TEXT NOT NULL,
    text        TEXT NOT NULL,
    persona     TEXT,
    private     INTEGER NOT NULL DEFAULT 0,
    at          TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_turns_session ON turns(session, seq);
CREATE TABLE IF NOT EXISTS persona_changes (
    at          TEXT NOT NULL,
    persona     TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS state (
    key         TEXT PRIMARY KEY,
    value       TEXT NOT NULL
);
"""

FTS_SCHEMA = """
CREATE VIRTUAL TABLE IF NOT EXISTS memories_fts USING fts5(
    content, tags, content='memories', content_rowid='rowid',
    tokenize='unicode61 remove_diacritics 2'
);
CREATE TRIGGER IF NOT EXISTS memories_ai AFTER INSERT ON memories BEGIN
    INSERT INTO memories_fts(rowid, content, tags)
    VALUES (new.rowid, new.content, new.tags);
END;
CREATE TRIGGER IF NOT EXISTS memories_ad AFTER DELETE ON memories BEGIN
    INSERT INTO memories_fts(memories_fts, rowid, content, tags)
    VALUES ('delete', old.rowid, old.content, old.tags);
END;
CREATE VIRTUAL TABLE IF NOT EXISTS turns_fts USING fts5(
    text, content='turns', content_rowid='rowid',
    tokenize='unicode61 remove_diacritics 2'
);
CREATE TRIGGER IF NOT EXISTS turns_ai AFTER INSERT ON turns BEGIN
    INSERT INTO turns_fts(rowid, text) VALUES (new.rowid, new.text);
END;
CREATE TRIGGER IF NOT EXISTS turns_ad AFTER DELETE ON turns BEGIN
    INSERT INTO turns_fts(turns_fts, rowid, text) VALUES ('delete', old.rowid, old.text);
END;
"""


class Store:
    def __init__(self, root: Path) -> None:
        self.root = root
        self.path = root / "memory.db"
        self.conn = sqlite3.connect(str(self.path))
        self.conn.row_factory = sqlite3.Row
        self.conn.executescript(SCHEMA)
        try:
            self.conn.executescript(FTS_SCHEMA)
            self.fts = True
        except sqlite3.OperationalError:
            self.fts = False  # SQLite built without FTS5: LIKE search instead
        self.conn.commit()

    def get(self, key: str, default=None):
        row = self.conn.execute("SELECT value FROM state WHERE key=?", (key,)).fetchone()
        return json.loads(row[0]) if row else default

    def put(self, key: str, value) -> None:
        self.conn.execute(
            "INSERT INTO state(key, value) VALUES(?, ?) "
            "ON CONFLICT(key) DO UPDATE SET value=excluded.value",
            (key, json.dumps(value)),
        )
        self.conn.commit()

    def graph(self) -> KnowledgeGraph:
        return KnowledgeGraph(str(self.path))


# ------------------------------------------------------------------ helpers


def slugify(text: str) -> str:
    text = unicodedata.normalize("NFKD", text).encode("ascii", "ignore").decode()
    text = re.sub(r"[^a-z0-9]+", "-", text.lower()).strip("-")
    return re.sub(r"-+", "-", text)[:48]


def fold(text: str) -> str:
    """Lowercase, accents removed: what keyword matching compares."""
    text = unicodedata.normalize("NFKD", text).encode("ascii", "ignore").decode()
    return text.lower()


def split_list(raw: str | None) -> list[str]:
    if not raw:
        return []
    return [p.strip() for p in raw.split(",") if p.strip()]


def parse_duration(raw: str) -> timedelta:
    m = re.fullmatch(r"\s*(\d+)\s*([mhdw])\s*", raw)
    if not m:
        raise SystemExit(f"bad duration {raw!r}: use 30m, 2h, 3d or 1w")
    n, unit = int(m.group(1)), m.group(2)
    return {"m": timedelta(minutes=n), "h": timedelta(hours=n),
            "d": timedelta(days=n), "w": timedelta(weeks=n)}[unit]


def importance(raw: str) -> int:
    """1-5; a 0-1 fraction (what agents often send) is scaled to it."""
    try:
        value = float(raw)
    except ValueError:
        raise argparse.ArgumentTypeError("importance is a number from 1 to 5")
    if 0 < value < 1:
        value *= 5
    return max(1, min(5, round(value)))


def every_delta(every: str) -> timedelta:
    return {"daily": timedelta(days=1), "weekly": timedelta(weeks=1)}.get(
        every
    ) or parse_duration(every)


def fts_query(text: str) -> str:
    """Words as quoted FTS5 terms joined by OR: no operator in the input runs."""
    words = re.findall(r"\w+", text, flags=re.UNICODE)
    return " OR ".join(f'"{w}"' for w in words[:16])


def emit(args, data, text: str) -> None:
    print(json.dumps(data, ensure_ascii=False, indent=2) if getattr(args, "json", False) else text)


# -------------------------------------------------------------- frontmatter


def parse_frontmatter(doc: str) -> tuple[dict, str]:
    """The YAML subset SOUL.md and SKILL.md use: scalars, flow or block
    lists, one level of nested maps. Unknown shapes are kept as strings."""
    if not doc.startswith("---"):
        return {}, doc
    end = doc.find("\n---", 3)
    if end < 0:
        return {}, doc
    head, body = doc[3:end], doc[end + 4:].lstrip("\n")
    data: dict = {}
    opener: str | None = None  # top-level key whose value is on the next lines
    nested: str | None = None  # key inside `opener`'s map awaiting list items
    lines = head.splitlines()
    i = 0
    while i < len(lines):
        line = lines[i]
        i += 1
        if not line.strip() or line.lstrip().startswith("#"):
            continue
        indent = len(line) - len(line.lstrip())
        stripped = line.strip()
        if stripped == "-" or stripped.startswith("- "):
            item = _scalar(stripped[1:])
            if opener and nested and isinstance(data[opener], dict):
                data[opener][nested].append(item)
            elif opener:
                if data[opener] == {}:
                    data[opener] = []
                if isinstance(data[opener], list):
                    data[opener].append(item)
            continue
        if ":" not in stripped:
            continue
        key, _, value = stripped.partition(":")
        key, value = key.strip(), value.strip()
        if value in ("|", ">", "|-", ">-"):
            # Block scalar: the following more-indented lines.
            block = []
            while i < len(lines) and (not lines[i].strip() or
                                      len(lines[i]) - len(lines[i].lstrip()) > indent):
                block.append(lines[i].strip())
                i += 1
            joiner = "\n" if value.startswith("|") else " "
            text = joiner.join(block).strip()
            if indent == 0:
                data[key], opener, nested = text, None, None
            elif opener and isinstance(data[opener], dict):
                data[opener][key] = text
            continue
        if indent == 0:
            nested = None
            if value == "":
                data[key], opener = {}, key
            else:
                data[key], opener = _scalar(value), None
        elif opener and isinstance(data[opener], dict):
            if value == "":
                data[opener][key], nested = [], key
            else:
                data[opener][key], nested = _scalar(value), None
    return data, body


def _scalar(value: str):
    value = value.strip()
    if value.startswith("[") or value.startswith('"'):
        try:
            return json.loads(value)
        except json.JSONDecodeError:
            pass
    if value.startswith("[") and value.endswith("]"):
        return [_scalar(p) for p in value[1:-1].split(",") if p.strip()]
    if value.startswith("'") and value.endswith("'"):
        return value[1:-1]
    if value in ("true", "false"):
        return value == "true"
    if value in ("null", "~"):
        return None
    try:
        return float(value) if "." in value else int(value)
    except ValueError:
        return value


def dump_frontmatter(data: dict) -> str:
    lines = ["---"]
    for key, value in data.items():
        if isinstance(value, dict):
            lines.append(f"{key}:")
            for k, v in value.items():
                lines.append(f"  {k}: {json.dumps(v, ensure_ascii=False)}")
        else:
            lines.append(f"{key}: {json.dumps(value, ensure_ascii=False)}")
    lines.append("---")
    return "\n".join(lines) + "\n"


# ----------------------------------------------------------------- personas


def personas_dir(root: Path) -> Path:
    path = root / "personas"
    path.mkdir(exist_ok=True)
    return path


def load_personas(root: Path, store: Store) -> list[dict]:
    out = []
    for soul in sorted(personas_dir(root).glob("*/SOUL.md")):
        meta, body = parse_frontmatter(soul.read_text(encoding="utf-8"))
        bastion = meta.get("bastion") if isinstance(meta.get("bastion"), dict) else {}
        slug = soul.parent.name
        base = float(bastion.get("weight", 0.5) or 0.5)
        row = store.conn.execute(
            "SELECT weight FROM weights WHERE persona=? ORDER BY at DESC, rowid DESC LIMIT 1",
            (slug,),
        ).fetchone()
        out.append({
            "slug": slug,
            "name": meta.get("name", slug),
            "description": meta.get("description", ""),
            "tier": bastion.get("privacy_tier", "cloud-ok"),
            "base_weight": base,
            "weight": float(row[0]) if row else base,
            "keywords": [str(k) for k in meta.get("trigger_keywords") or []],
            "domains": [str(d) for d in meta.get("domains") or []],
            "skills": meta.get("skills") or [],
            "objectives": meta.get("objectives") or [],
            "goals": meta.get("goals") or [],
            "scope": meta.get("scope", ""),
            "body": body.strip(),
            "path": str(soul),
        })
    return out


def find_persona(personas: list[dict], key: str) -> dict:
    for p in personas:
        if key in (p["slug"], p["name"]) or slugify(key) == p["slug"]:
            return p
    raise SystemExit(f"no persona {key!r}; see: persona list")


def route(personas: list[dict], text: str, active: str | None) -> tuple[dict | None, float, str]:
    """Best persona for `text`: keyword hits count 1, domain hits 0.5, scaled
    by weight. No hit keeps the active persona."""
    folded = f" {fold(text)} "
    best, best_score, best_why = None, 0.0, ""
    for p in personas:
        hits = [k for k in p["keywords"] if re.search(rf"\b{re.escape(fold(k))}\b", folded)]
        dom = [d for d in p["domains"] if re.search(rf"\b{re.escape(fold(d))}\b", folded)]
        raw = len(hits) + 0.5 * len(dom)
        if raw == 0:
            continue
        score = raw * (0.5 + p["weight"])
        if p["slug"] == active:
            score += 0.25  # stickiness: don't flip on a tie
        if score > best_score:
            best, best_score = p, score
            best_why = "matched " + ", ".join(hits + dom)
    if best is None and active:
        for p in personas:
            if p["slug"] == active:
                return p, 0.0, "no match; stays active"
    return best, round(best_score, 3), best_why


# ------------------------------------------------------------------- memory


def mem_add(store: Store, content: str, kind="note", persona=None, tags="",
            importance=3, private=False, entities=None, relations=None) -> str:
    mid = "m-" + uuid.uuid4().hex[:8]
    store.conn.execute(
        "INSERT INTO memories(id, content, kind, persona, tags, importance, private, created_at) "
        "VALUES(?,?,?,?,?,?,?,?)",
        (mid, content, kind, persona, tags, max(1, min(5, importance)), int(private), iso(now())),
    )
    store.conn.commit()
    if entities or relations:
        kg = store.graph()
        ids = {}
        for ent in entities or []:
            name, _, etype = ent.partition(":")
            ids[name.strip()] = kg.upsert_entity(name.strip(), etype.strip() or "thing")
        for rel in relations or []:
            parts = rel.split()
            if len(parts) < 3:
                raise SystemExit(f"bad relation {rel!r}: use 'Source relation Target'")
            src, rtype, dst = parts[0], parts[1], " ".join(parts[2:])
            sid = ids.get(src) or kg.upsert_entity(src, "thing")
            did = ids.get(dst) or kg.upsert_entity(dst, "thing")
            kg.add_relation(sid, did, rtype, mid)
        kg.close()
    return mid


def mem_search(store: Store, query: str, persona=None, limit=5, include_private=False) -> list[dict]:
    """BM25 relevance, nudged by importance and recency."""
    where = ["m.valid_to IS NULL"]
    params: list = []
    if persona:
        where.append("(m.persona = ? OR m.persona IS NULL)")
        params.append(persona)
    if not include_private:
        where.append("m.private = 0")
    q = fts_query(query)
    if not q:
        return []
    if store.fts:
        sql = (
            "SELECT m.*, bm25(memories_fts) AS rank FROM memories_fts "
            "JOIN memories m ON m.rowid = memories_fts.rowid "
            f"WHERE memories_fts MATCH ? AND {' AND '.join(where)} ORDER BY rank LIMIT ?"
        )
        rows = store.conn.execute(sql, [q, *params, limit * 4]).fetchall()
    else:
        words = re.findall(r"\w+", query)[:8]
        like = " OR ".join("m.content LIKE ?" for _ in words)
        sql = f"SELECT m.*, 0 AS rank FROM memories m WHERE ({like}) AND {' AND '.join(where)} LIMIT ?"
        rows = store.conn.execute(sql, [*(f"%{w}%" for w in words), *params, limit * 4]).fetchall()
    scored = []
    for r in rows:
        age_days = (now() - parse_iso(r["created_at"])).days
        score = -float(r["rank"]) + 0.3 * r["importance"] - 0.01 * min(age_days, 365)
        scored.append((score, dict(r)))
    scored.sort(key=lambda s: s[0], reverse=True)
    hits = [m for _, m in scored[:limit]]
    for m in hits:
        store.conn.execute(
            "UPDATE memories SET uses = uses + 1, last_used = ? WHERE id = ?", (iso(now()), m["id"])
        )
    store.conn.commit()
    return hits


def mem_recall(store: Store, persona=None, limit=8, include_private=False) -> list[dict]:
    """What to know at the start of a session: important, then recent."""
    where = ["valid_to IS NULL"]
    params: list = []
    if persona:
        where.append("(persona = ? OR persona IS NULL)")
        params.append(persona)
    if not include_private:
        where.append("private = 0")
    rows = store.conn.execute(
        f"SELECT * FROM memories WHERE {' AND '.join(where)} "
        "ORDER BY importance DESC, COALESCE(last_used, created_at) DESC LIMIT ?",
        [*params, limit],
    ).fetchall()
    return [dict(r) for r in rows]


def fmt_mem(m: dict) -> str:
    who = f" @{m['persona']}" if m.get("persona") else ""
    lock = " [private]" if m.get("private") else ""
    return f"{m['id']} ({m['kind']}{who}, ★{m['importance']}){lock} {m['content']}"


# ---------------------------------------------------------------- reminders


def reminders_due(store: Store, horizon: timedelta = timedelta(0)) -> list[dict]:
    limit = iso(now() + horizon)
    rows = store.conn.execute(
        "SELECT * FROM reminders WHERE done_at IS NULL AND due_at <= ? ORDER BY due_at", (limit,)
    ).fetchall()
    return [dict(r) for r in rows]


def fmt_reminder(r: dict) -> str:
    due = parse_iso(r["due_at"]).astimezone().strftime("%Y-%m-%d %H:%M")
    rep = f" (every {r['every']})" if r.get("every") else ""
    who = f" @{r['persona']}" if r.get("persona") else ""
    late = " OVERDUE" if parse_iso(r["due_at"]) < now() else ""
    return f"{r['id']} {due}{rep}{who}{late} — {r['text']}"


def desktop_notify(title: str, body: str) -> bool:
    """Best effort: notify-send (Linux), osascript (macOS), else nothing."""
    if shutil.which("notify-send"):
        cmd = ["notify-send", "--app-name=Bastion", title, body]
    elif shutil.which("osascript"):
        script = f"display notification {json.dumps(body)} with title {json.dumps(title)}"
        cmd = ["osascript", "-e", script]
    else:
        return False
    return subprocess.run(cmd, capture_output=True, check=False).returncode == 0


# ------------------------------------------------------------------- skills


def skills_dir(root: Path) -> Path:
    path = Path(os.environ.get("BASTION_SKILLS_DIR") or root / "skills").expanduser()
    path.mkdir(parents=True, exist_ok=True)
    return path


def skill_path(root: Path, name: str) -> Path:
    slug = slugify(name)
    if not slug:
        raise SystemExit("skill name needs letters or digits")
    return skills_dir(root) / slug / "SKILL.md"


def load_skills(root: Path, store: Store) -> list[dict]:
    out = []
    for f in sorted(skills_dir(root).glob("*/SKILL.md")):
        meta, _ = parse_frontmatter(f.read_text(encoding="utf-8"))
        name = f.parent.name
        runs = store.conn.execute(
            "SELECT outcome, COUNT(*) FROM skill_runs WHERE skill=? GROUP BY outcome", (name,)
        ).fetchall()
        counts = {o: c for o, c in runs}
        # A failure counts until an improvement comes after it.
        last = store.conn.execute(
            "SELECT outcome, note FROM skill_runs WHERE skill=? AND outcome IN ('fail', 'improved') "
            "ORDER BY at DESC, rowid DESC LIMIT 1",
            (name,),
        ).fetchone()
        out.append({
            "name": name,
            "description": meta.get("description", ""),
            "version": (meta.get("metadata") or {}).get("version", "1")
            if isinstance(meta.get("metadata"), dict) else "1",
            "ok": counts.get("ok", 0),
            "fail": counts.get("fail", 0),
            "open_fail": last[1] if last and last[0] == "fail" else None,
            "path": str(f),
        })
    return out


SKILL_TEMPLATE = """# {title}

## When to use
{when}

## Steps
1. …

## Pitfalls
- …

## Verification
- How to tell it worked.
"""


# ----------------------------------------------------------------- sessions


def turn_add(store: Store, session: str, role: str, text: str, persona=None,
             private=False, turn_id=None, at=None) -> bool:
    """Stores one conversation turn; an id seen before is skipped."""
    text = text.strip()
    if not text:
        return False
    seq = store.conn.execute(
        "SELECT COALESCE(MAX(seq), 0) + 1 FROM turns WHERE session=?", (session,)
    ).fetchone()[0]
    cur = store.conn.execute(
        "INSERT OR IGNORE INTO turns(id, session, seq, role, text, persona, private, at) "
        "VALUES(?,?,?,?,?,?,?,?)",
        (turn_id or "t-" + uuid.uuid4().hex[:10], session, seq, role, text[:TURN_MAX],
         persona, int(private), at or iso(now())),
    )
    return cur.rowcount > 0


def transcript_turns(path: Path, offset: int) -> tuple[list[dict], int]:
    """User prompts and assistant replies from a Claude Code transcript, read
    from byte `offset` on. Tool calls, tool results, thinking, subagents and
    meta lines are left out: what was said, not how it was done."""
    turns = []
    with path.open("rb") as f:
        f.seek(offset)
        data = f.read()
    end = data.rfind(b"\n") + 1  # a half-written last line waits for next time
    for raw in data[:end].splitlines():
        try:
            line = json.loads(raw)
        except (json.JSONDecodeError, UnicodeDecodeError):
            continue
        if line.get("type") not in ("user", "assistant") or line.get("isSidechain") \
                or line.get("isMeta"):
            continue
        content = (line.get("message") or {}).get("content")
        if isinstance(content, str):
            text = content
        elif isinstance(content, list):
            text = "\n".join(b.get("text", "") for b in content
                             if isinstance(b, dict) and b.get("type") == "text")
        else:
            continue
        if not text.strip() or text.lstrip().startswith(HARNESS_TAGS):
            continue  # slash-command echoes, hook output, system reminders
        turns.append({"id": line.get("uuid"), "role": line["type"], "text": text,
                      "at": line.get("timestamp")})
    return turns, offset + end


def turns_search(store: Store, query: str, limit=5, include_private=False) -> list[dict]:
    q = fts_query(query)
    if not q:
        return []
    private = "" if include_private else "AND t.private = 0"
    if store.fts:
        rows = store.conn.execute(
            "SELECT t.*, snippet(turns_fts, 0, '[', ']', '…', 16) AS snip FROM turns_fts "
            f"JOIN turns t ON t.rowid = turns_fts.rowid WHERE turns_fts MATCH ? {private} "
            "ORDER BY bm25(turns_fts) LIMIT ?", (q, limit)).fetchall()
    else:
        words = re.findall(r"\w+", query)[:8]
        like = " OR ".join("t.text LIKE ?" for _ in words)
        rows = store.conn.execute(
            f"SELECT t.*, substr(t.text, 1, 160) AS snip FROM turns t WHERE ({like}) {private} "
            "ORDER BY t.at DESC LIMIT ?", [*(f"%{w}%" for w in words), limit]).fetchall()
    return [dict(r) for r in rows]


def fmt_turn(t: dict, full=False) -> str:
    text = t["text"] if full else (t.get("snip") or t["text"][:160])
    text = " ".join(text.split())
    lock = " [private]" if t.get("private") else ""
    return f"{t['session'][:8]}#{t['seq']} {t['at'][:16]} {t['role']}{lock}: {text}"


# ---------------------------------------------------------------- companion

FACES = {"happy": "(^‿^)", "neutral": "(•_•)", "worried": "(°□°)", "sleepy": "(-_-)zz"}


def gain_xp(store: Store, amount: int) -> None:
    pet = store.get("buddy", {"name": "Bastion", "xp": 0})
    pet["xp"] = pet.get("xp", 0) + amount
    pet["last_seen"] = iso(now())
    store.put("buddy", pet)


def buddy_state(store: Store) -> dict:
    pet = store.get("buddy", {"name": "Bastion", "xp": 0})
    overdue = len(reminders_due(store))
    last = pet.get("last_seen")
    idle = (now() - parse_iso(last)).days if last else 0
    if overdue:
        mood = "worried"
    elif idle >= 3:
        mood = "sleepy"
    elif last and (now() - parse_iso(last)) < timedelta(hours=6):
        mood = "happy"
    else:
        mood = "neutral"
    xp = pet.get("xp", 0)
    return {"name": pet.get("name", "Bastion"), "mood": mood, "face": FACES[mood],
            "xp": xp, "level": xp // 50 + 1, "overdue": overdue}


# -------------------------------------------------------------------- brief


def brief(root: Path, store: Store, budget: int | None = None) -> str:
    """What an agent reads at session start to act as Bastion."""
    personas = load_personas(root, store)
    active = store.get("active_persona")
    lines = [f"[bastion] pack {VERSION}, home {root}"]
    if personas:
        lines.append("Personas (act as the active one; route with `persona route`):")
        for p in personas:
            mark = "*" if p["slug"] == active else "-"
            lines.append(f" {mark} {p['slug']} w={p['weight']:.2f} {p['tier']}: {p['description']}")
    else:
        lines.append("No personas yet: offer to create some (skill `personas`).")
    due = reminders_due(store, timedelta(hours=12))
    if due:
        lines.append("Reminders due within 12h — tell the person:")
        lines += [f" - {fmt_reminder(r)}" for r in due[:8]]
    mems = mem_recall(store, active, limit=6)
    if mems:
        lines.append("Remember:")
        lines += [f" - {m['content']}" for m in mems]
    skills = load_skills(root, store)
    if skills:
        lines.append(f"Learned skills (read {skills_dir(root)}/<name>/SKILL.md when relevant):")
        lines += [f" - {s['name']}: {s['description']}" for s in skills[:20]]
        weak = [f"{s['name']} ({s['open_fail'] or 'failed'})" for s in skills
                if s["open_fail"] is not None]
        if weak:
            lines.append("Skills with failures to improve: " + ", ".join(weak[:5]))
    text = "\n".join(lines)
    if budget and len(text) > budget:
        text = text[: budget - 1] + "…"
    return text


def install_cli(root: Path) -> Path:
    """Copies this CLI to ``<home>/bin`` so a status line or cron job has a
    path that survives plugin updates. Rewritten only when it changed."""
    here = Path(__file__).resolve().parent
    dest = root / "bin"
    dest.mkdir(exist_ok=True)
    for name in ("bastion.py", "knowledge_graph.py"):
        src, dst = here / name, dest / name
        if src != dst and (not dst.exists() or dst.read_bytes() != src.read_bytes()):
            shutil.copy2(src, dst)
    return dest / "bastion.py"


# ---------------------------------------------------------------- commands


def cmd_init(args, root, store):
    personas_dir(root)
    skills_dir(root)
    pet = store.get("buddy")
    if pet is None:
        store.put("buddy", {"name": args.name or "Bastion", "xp": 0, "last_seen": iso(now())})
    elif args.name:
        pet["name"] = args.name
        store.put("buddy", pet)
    cli = install_cli(root)
    print(f"Bastion home ready: {root} (fts5={'on' if store.fts else 'off'})\n"
          f"Stable CLI path for a status line or cron: python3 {cli}")


def cmd_brief(args, root, store):
    print(brief(root, store))


def cmd_mem(args, root, store):
    if args.action == "add":
        # Global unless a persona is named: a preference about the person
        # holds for every persona.
        persona = args.persona
        private = args.private
        if persona and not private:
            match = [p for p in load_personas(root, store) if p["slug"] == persona]
            private = bool(match and match[0]["tier"] == "local-only")
        mid = mem_add(store, args.text, args.kind, persona, ",".join(split_list(args.tags)),
                      args.importance, private, split_list(args.entities), args.relation)
        gain_xp(store, 2)
        emit(args, {"id": mid}, f"remembered {mid}")
    elif args.action == "search":
        hits = mem_search(store, args.text, args.persona, args.limit, args.include_private)
        emit(args, hits, "\n".join(fmt_mem(m) for m in hits) or "nothing found")
    elif args.action == "recall":
        mems = mem_recall(store, args.persona or store.get("active_persona"), args.limit,
                          args.include_private)
        emit(args, mems, "\n".join(fmt_mem(m) for m in mems) or "no memories yet")
    elif args.action == "forget":
        cur = store.conn.execute(
            "UPDATE memories SET valid_to=? WHERE id=? AND valid_to IS NULL", (iso(now()), args.text)
        )
        store.conn.commit()
        if cur.rowcount:
            kg = store.graph()
            kg.invalidate_by_memory(args.text)
            kg.close()
        print("forgotten" if cur.rowcount else f"no active memory {args.text}")
    elif args.action == "graph":
        kg = store.graph()
        row = kg._conn.execute(
            "SELECT id FROM entities WHERE name=? AND valid_to IS NULL", (args.text,)
        ).fetchone()
        if not row:
            print(f"no entity {args.text!r}")
            return
        names = dict(kg._conn.execute("SELECT id, name FROM entities").fetchall())
        rels = kg.get_relations(row[0])
        kg.close()
        print("\n".join(
            f"{names.get(r.source_id)} —{r.relation_type}→ {names.get(r.target_id)} ({r.memory_id})"
            for r in rels
        ) or "no relations")
    elif args.action == "stats":
        total = store.conn.execute("SELECT COUNT(*) FROM memories WHERE valid_to IS NULL").fetchone()[0]
        print(f"{total} memories in {store.path} (fts5={'on' if store.fts else 'off'})")


def cmd_persona(args, root, store):
    personas = load_personas(root, store)
    active = store.get("active_persona")
    if args.action == "list":
        emit(args, personas, "\n".join(
            f"{'*' if p['slug'] == active else ' '} {p['slug']} w={p['weight']:.2f} "
            f"{p['tier']} [{', '.join(p['keywords'])}] {p['description']}"
            for p in personas) or "no personas")
    elif args.action == "new":
        if not args.name:
            raise SystemExit("persona new needs --name")
        slug = slugify(args.name)
        soul = personas_dir(root) / slug / "SOUL.md"
        if soul.exists() and not args.force:
            raise SystemExit(f"{slug} exists; edit {soul} or pass --force")
        if args.tier not in TIERS:
            raise SystemExit(f"--tier must be one of {', '.join(TIERS)}")
        weight = max(0.0, min(1.0, args.weight))
        meta = {
            "name": args.name,
            "description": args.description or "",
            "bastion": {"privacy_tier": args.tier, "weight": weight},
            "trigger_keywords": split_list(args.keywords),
            "domains": split_list(args.domains),
            "skills": split_list(args.skills),
            "objectives": split_list(args.objectives),
            "goals": split_list(args.goals),
            "scope": args.scope or "",
            "created_at": iso(now()),
        }
        body = args.body or (
            f"You are {args.name}, one of this person's Bastion personas.\n\n"
            f"{args.description or ''}\n\nVoice: {args.voice or 'direct and warm'}.\n"
            "Stay inside your scope; hand off to another persona when a request is not yours."
        )
        soul.parent.mkdir(parents=True, exist_ok=True)
        soul.write_text(dump_frontmatter(meta) + "\n" + body.strip() + "\n", encoding="utf-8")
        if not active:
            set_active(store, slug)
        print(f"created {soul}")
    elif args.action == "route":
        p, score, why = route(personas, args.text, active)
        if p and args.use and p["slug"] != active:
            set_active(store, p["slug"])
        data = {"persona": p["slug"] if p else None, "score": score, "why": why}
        emit(args, data, f"{p['slug']} ({why}, score {score})" if p else "no persona matches")
    elif args.action == "use":
        p = find_persona(personas, args.text)
        set_active(store, p["slug"])
        print(f"active persona: {p['slug']}\n\n{p['body']}")
    elif args.action == "show":
        p = find_persona(personas, args.text or active or "")
        emit(args, p, Path(p["path"]).read_text(encoding="utf-8"))
    elif args.action == "weight":
        p = find_persona(personas, args.text)
        if args.value is None or not args.why:
            raise SystemExit("persona weight SLUG --value 0.7 --why 'reason'")
        value = max(0.0, min(1.0, args.value))
        store.conn.execute(
            "INSERT INTO weights(persona, weight, why, at) VALUES(?,?,?,?)",
            (p["slug"], value, args.why, iso(now())),
        )
        store.conn.commit()
        print(f"{p['slug']}: {p['weight']:.2f} → {value:.2f} ({args.why})")
    elif args.action == "history":
        p = find_persona(personas, args.text)
        rows = store.conn.execute(
            "SELECT weight, why, at FROM weights WHERE persona=? ORDER BY at", (p["slug"],)
        ).fetchall()
        print(f"base {p['base_weight']:.2f}")
        print("\n".join(f"{r['at']} → {r['weight']:.2f}: {r['why']}" for r in rows))
    elif args.action == "check":
        problems = []
        for p in personas:
            for field in ("description", "objectives", "goals", "scope"):
                if not p[field]:
                    problems.append(f"{p['slug']}: missing {field}")
            if p["tier"] not in TIERS:
                problems.append(f"{p['slug']}: privacy_tier must be one of {', '.join(TIERS)}")
            if not p["keywords"]:
                problems.append(f"{p['slug']}: no trigger_keywords, routing never picks it")
        print("\n".join(problems) or f"{len(personas)} personas ok")


def cmd_skill(args, root, store):
    if args.action == "list":
        skills = load_skills(root, store)
        emit(args, skills, "\n".join(
            f"{s['name']} v{s['version']} ok={s['ok']} fail={s['fail']}: {s['description']}"
            for s in skills) or "no learned skills yet")
        return
    path = skill_path(root, args.name)
    name = path.parent.name
    if args.action == "new":
        if path.exists():
            raise SystemExit(f"{name} exists: improve it instead ({path})")
        if not args.description:
            raise SystemExit("skill new needs --description (when to use it, in one line)")
        meta = {"name": name, "description": args.description,
                "metadata": {"version": "1", "author": "bastion-pack",
                             "persona": args.persona or store.get("active_persona") or ""}}
        title = name.replace("-", " ").capitalize()
        body = SKILL_TEMPLATE.format(title=title, when=args.description)
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(dump_frontmatter(meta) + "\n" + body, encoding="utf-8")
        gain_xp(store, 10)
        print(f"created {path}: fill the steps from what just worked")
    elif args.action == "log":
        if not path.exists():
            raise SystemExit(f"no skill {name}")
        store.conn.execute(
            "INSERT INTO skill_runs(skill, outcome, note, at) VALUES(?,?,?,?)",
            (name, args.outcome, args.note or "", iso(now())),
        )
        store.conn.commit()
        if args.outcome == "ok":
            gain_xp(store, 3)
        hint = "; patch the skill now (skill snapshot, edit, skill commit)" if args.outcome == "fail" else ""
        print(f"logged {args.outcome} for {name}{hint}")
    elif args.action == "snapshot":
        if not path.exists():
            raise SystemExit(f"no skill {name}")
        versions = path.parent / ".versions"
        versions.mkdir(exist_ok=True)
        n = len(list(versions.glob("v*.md"))) + 1
        shutil.copy2(path, versions / f"v{n}.md")
        print(f"saved {versions / f'v{n}.md'}; edit {path}, then: skill commit {name} --note '...'")
    elif args.action == "commit":
        if not path.exists():
            raise SystemExit(f"no skill {name}")
        meta, body = parse_frontmatter(path.read_text(encoding="utf-8"))
        md = meta.get("metadata") if isinstance(meta.get("metadata"), dict) else {}
        version = str(int(str(md.get("version", "1")).split(".")[0]) + 1)
        md["version"] = version
        meta["metadata"] = md
        path.write_text(dump_frontmatter(meta) + "\n" + body, encoding="utf-8")
        store.conn.execute(
            "INSERT INTO skill_runs(skill, outcome, note, at) VALUES(?,?,?,?)",
            (name, "improved", args.note or "", iso(now())),
        )
        store.conn.commit()
        gain_xp(store, 5)
        print(f"{name} is now v{version}")
    elif args.action == "rollback":
        versions = sorted((path.parent / ".versions").glob("v*.md"),
                          key=lambda p: int(p.stem[1:]))
        if not versions:
            raise SystemExit(f"{name} has no snapshots")
        pick = versions[-1] if args.to is None else path.parent / ".versions" / f"v{args.to}.md"
        if not pick.exists():
            raise SystemExit(f"no snapshot {pick.name}")
        shutil.copy2(pick, path)
        print(f"{name} restored from {pick.name}")
    elif args.action == "link":
        target = Path(args.target).expanduser() / name
        target.parent.mkdir(parents=True, exist_ok=True)
        if target.exists() or target.is_symlink():
            raise SystemExit(f"{target} exists")
        target.symlink_to(path.parent, target_is_directory=True)
        print(f"linked {target} → {path.parent}")


def cmd_remind(args, root, store):
    if args.action == "add":
        if args.at:
            due = parse_iso(args.at)
        elif args.in_:
            due = now() + parse_duration(args.in_)
        else:
            raise SystemExit("remind add needs --at 'YYYY-MM-DD HH:MM' or --in 2h")
        if args.every:
            every_delta(args.every)  # validate
        rid = "r-" + uuid.uuid4().hex[:6]
        store.conn.execute(
            "INSERT INTO reminders(id, text, due_at, every, persona, created_at) VALUES(?,?,?,?,?,?)",
            (rid, args.text, iso(due), args.every, args.persona or store.get("active_persona"),
             iso(now())),
        )
        store.conn.commit()
        print(f"{rid} set for {due.astimezone().strftime('%Y-%m-%d %H:%M')}")
    elif args.action in ("list", "due"):
        if args.action == "due":
            rows = reminders_due(store, parse_duration(args.within) if args.within else timedelta(0))
        else:
            rows = [dict(r) for r in store.conn.execute(
                "SELECT * FROM reminders WHERE done_at IS NULL ORDER BY due_at").fetchall()]
        emit(args, rows, "\n".join(fmt_reminder(r) for r in rows) or "nothing pending")
    elif args.action == "done":
        row = store.conn.execute("SELECT * FROM reminders WHERE id=?", (args.text,)).fetchone()
        if not row or row["done_at"]:
            raise SystemExit(f"no pending reminder {args.text}")
        if row["every"]:
            nxt = parse_iso(row["due_at"])
            step = every_delta(row["every"])
            while nxt <= now():
                nxt += step
            store.conn.execute("UPDATE reminders SET due_at=?, shown_at=NULL WHERE id=?",
                               (iso(nxt), row["id"]))
            msg = f"done; next {nxt.astimezone().strftime('%Y-%m-%d %H:%M')}"
        else:
            store.conn.execute("UPDATE reminders SET done_at=? WHERE id=?", (iso(now()), row["id"]))
            msg = "done"
        store.conn.commit()
        gain_xp(store, 3)
        print(msg)
    elif args.action == "snooze":
        due = now() + parse_duration(args.in_ or "1h")
        cur = store.conn.execute(
            "UPDATE reminders SET due_at=?, shown_at=NULL WHERE id=? AND done_at IS NULL",
            (iso(due), args.text),
        )
        store.conn.commit()
        print("snoozed" if cur.rowcount else f"no pending reminder {args.text}")
    elif args.action == "notify":
        sent = 0
        for r in reminders_due(store):
            if r["shown_at"]:
                continue
            if desktop_notify("Bastion", r["text"]):
                sent += 1
            store.conn.execute("UPDATE reminders SET shown_at=? WHERE id=?", (iso(now()), r["id"]))
        store.conn.commit()
        print(f"{sent} notification(s) sent")


def cmd_cabinet(args, root, store):
    personas = load_personas(root, store)
    chosen = [find_persona(personas, s) for s in args.personas] if args.personas else personas
    if len(chosen) < 2:
        raise SystemExit("Cabinet needs at least two personas")
    total = sum(p["weight"] for p in chosen) or 1.0
    lines = [f"Cabinet on: {args.question or '(state the question)'}", ""]
    for p in sorted(chosen, key=lambda p: -p["weight"]):
        lines.append(f"## {p['name']} ({p['slug']}, weight {p['weight']:.2f} = "
                     f"{p['weight'] / total:.0%} of the vote, {p['tier']})")
        if p["objectives"]:
            lines.append("Objectives: " + "; ".join(map(str, p["objectives"])))
        if p["scope"]:
            lines.append(f"Out of scope: {p['scope']}")
        mems = mem_search(store, args.question, p["slug"], 3) if args.question else []
        lines += [f"Knows: {m['content']}" for m in mems if m.get("persona") == p["slug"]]
        lines.append("")
    lines.append("Each persona argues its position in its own voice; keep dissent; "
                 "then synthesize a weighted recommendation and save it: "
                 "mem add '<decision>' --kind decision --importance 4")
    print("\n".join(lines))


def cmd_session(args, root, store):
    if args.action == "search":
        hits = turns_search(store, args.text, args.limit, args.include_private)
        emit(args, hits, "\n".join(fmt_turn(t) for t in hits) or "nothing found")
    elif args.action == "list":
        inner = "" if args.include_private else "AND f.private = 0"
        outer = "" if args.include_private else "WHERE t.private = 0"
        rows = [dict(r) for r in store.conn.execute(
            "SELECT session, MIN(at) AS started, MAX(at) AS ended, COUNT(*) AS turns, "
            "(SELECT text FROM turns f WHERE f.session = t.session AND f.role = 'user' "
            f" {inner} ORDER BY seq LIMIT 1) AS first FROM turns t {outer} GROUP BY session "
            "ORDER BY ended DESC LIMIT ?", (args.limit,)).fetchall()]
        emit(args, rows, "\n".join(
            f"{r['session'][:8]} {r['started'][:16]} {r['turns']} turns: "
            f"{' '.join((r['first'] or '').split())[:80]}" for r in rows) or "no sessions yet")
    elif args.action == "show":
        # `abc12345` or `abc12345#14` (a hit from search): the turns around it.
        ref, _, at = args.text.partition("#")
        row = store.conn.execute(
            "SELECT session FROM turns WHERE session LIKE ? LIMIT 1", (ref + "%",)
        ).fetchone()
        if not row:
            raise SystemExit(f"no session {ref}")
        center = int(at) if at.isdigit() else 1
        lo = max(1, center - args.around) if at else 1
        hi = center + args.around if at else lo + 2 * args.around
        private = "" if args.include_private else "AND private = 0"
        rows = [dict(r) for r in store.conn.execute(
            f"SELECT * FROM turns WHERE session=? AND seq BETWEEN ? AND ? {private} ORDER BY seq",
            (row[0], lo, hi)).fetchall()]
        emit(args, rows, "\n\n".join(fmt_turn(t, full=True) for t in rows) or "nothing there")
    elif args.action == "add":
        if not args.session or args.role not in ("user", "assistant"):
            raise SystemExit("session add TEXT --session ID --role user|assistant")
        persona = store.get("active_persona")
        turn_add(store, args.session, args.role, args.text, persona,
                 args.private or persona_private(root, store, persona))
        store.conn.commit()
        print("logged")
    elif args.action == "forget":
        cur = store.conn.execute("DELETE FROM turns WHERE session LIKE ?", (args.text + "%",))
        store.conn.commit()
        print(f"{cur.rowcount} turns deleted")
    elif args.action == "prune":
        cutoff = iso(now() - parse_duration(args.older or "180d"))
        cur = store.conn.execute("DELETE FROM turns WHERE at < ?", (cutoff,))
        store.conn.commit()
        print(f"{cur.rowcount} turns older than {args.older or '180d'} deleted")


def set_active(store: Store, slug: str) -> None:
    """Makes `slug` the active persona and logs when, so a turn stored later
    is attributed to the persona active when it was said."""
    store.put("active_persona", slug)
    store.conn.execute("INSERT INTO persona_changes(at, persona) VALUES(?, ?)",
                       (iso(now()), slug))
    store.conn.commit()


def persona_at(store: Store, at) -> str | None:
    """The persona active at time `at`, or None when unknown."""
    if not at:
        return None
    try:
        when = iso(parse_iso(str(at).replace("Z", "+00:00")))
    except ValueError:
        return None
    row = store.conn.execute(
        "SELECT persona FROM persona_changes WHERE at <= ? ORDER BY at DESC, rowid DESC LIMIT 1",
        (when,)).fetchone()
    return row[0] if row else None


def persona_private(root: Path, store: Store, slug) -> bool:
    if not slug:
        return False
    return any(p["slug"] == slug and p["tier"] == "local-only"
               for p in load_personas(root, store))


def cmd_buddy(args, root, store):
    if args.rename:
        pet = store.get("buddy", {"name": "Bastion", "xp": 0})
        pet["name"] = args.rename
        store.put("buddy", pet)
    s = buddy_state(store)
    active = store.get("active_persona") or "—"
    due = f" · {s['overdue']} due" if s["overdue"] else ""
    emit(args, {**s, "persona": active},
         f"{s['face']} {s['name']} L{s['level']} · {active}{due}")


def cmd_hook(args, root, store):
    """Claude Code hooks (stdin = hook JSON; stdout = added context)."""
    try:
        event = json.load(sys.stdin) if not sys.stdin.isatty() else {}
    except json.JSONDecodeError:
        event = {}
    if args.event == "stop":
        # Keeps what was said this turn, for `session search`.
        path = Path(str(event.get("transcript_path") or "")).expanduser()
        session = str(event.get("session_id") or path.stem)
        if not path.is_file():
            return
        key = f"transcript_offset:{path}"
        offset = store.get(key, 0)
        if offset > path.stat().st_size:
            offset = 0  # rewritten transcript: ids keep it from duplicating
        turns, offset = transcript_turns(path, offset)
        local_only = {p["slug"] for p in load_personas(root, store) if p["tier"] == "local-only"}
        for t in turns:
            persona = persona_at(store, t["at"])
            # Fails closed: with any local-only persona around, a turn whose
            # persona can't be told (no time, before the first switch) is private.
            private = bool(local_only) and (persona is None or persona in local_only)
            turn_add(store, session, t["role"], t["text"], persona, private, t["id"], t["at"])
        store.conn.commit()
        store.put(key, offset)
        return
    if args.event == "session-start":
        pet = store.get("buddy")
        if pet is None:
            store.put("buddy", {"name": "Bastion", "xp": 0, "last_seen": iso(now())})
        install_cli(root)
        print(brief(root, store, budget=HOOK_BUDGET * 2))
        return
    prompt = str(event.get("prompt") or "")
    personas = load_personas(root, store)
    active = store.get("active_persona")
    out = []
    p, _, why = route(personas, prompt, active)
    if p and p["slug"] != active:
        set_active(store, p["slug"])
        out.append(f"[bastion] switch to persona {p['slug']} ({why}). {p['description']}")
    for m in mem_search(store, prompt, p["slug"] if p else active, 3) if prompt else []:
        out.append(f"[bastion] memory: {m['content']}")
    fresh = [r for r in reminders_due(store) if not r["shown_at"] or
             now() - parse_iso(r["shown_at"]) > timedelta(minutes=30)]
    for r in fresh[:3]:
        out.append(f"[bastion] due, mention it: {r['text']} ({r['id']})")
        store.conn.execute("UPDATE reminders SET shown_at=? WHERE id=?", (iso(now()), r["id"]))
    store.conn.commit()
    pet = store.get("buddy", {"name": "Bastion", "xp": 0})
    pet["last_seen"] = iso(now())
    store.put("buddy", pet)
    text = "\n".join(out)
    if text:
        print(text[:HOOK_BUDGET])


# --------------------------------------------------------------------- main


def build_parser() -> argparse.ArgumentParser:
    ap = argparse.ArgumentParser(prog="bastion.py", description="Bastion pack (no daemon)")
    ap.add_argument("--version", action="version", version=VERSION)
    sub = ap.add_subparsers(dest="group", required=True)

    s = sub.add_parser("init")
    s.add_argument("--name", help="the companion's name")
    s.set_defaults(fn=cmd_init)
    sub.add_parser("brief").set_defaults(fn=cmd_brief)

    s = sub.add_parser("mem")
    s.add_argument("action", choices=["add", "search", "recall", "forget", "graph", "stats"])
    s.add_argument("text", nargs="?", default="")
    s.add_argument("--kind", choices=KINDS, default="note")
    s.add_argument("--persona")
    s.add_argument("--tags")
    s.add_argument("--importance", type=importance, default=3, help="1-5 (or 0-1, scaled)")
    s.add_argument("--private", action="store_true", help="never shown by hooks or Cabinet")
    s.add_argument("--include-private", action="store_true")
    s.add_argument("--entities", help="Name:type,Name:type")
    s.add_argument("--relation", action="append", help="'Source relation Target' (repeatable)")
    s.add_argument("--limit", type=int, default=5)
    s.add_argument("--json", action="store_true")
    s.set_defaults(fn=cmd_mem)

    s = sub.add_parser("persona")
    s.add_argument("action", choices=["list", "new", "route", "use", "show", "weight",
                                      "history", "check"])
    s.add_argument("text", nargs="?", default="")
    for flag in ("--name", "--description", "--keywords", "--domains", "--skills",
                 "--objectives", "--goals", "--scope", "--voice", "--body", "--why"):
        s.add_argument(flag)
    s.add_argument("--tier", default="cloud-ok")
    s.add_argument("--weight", type=float, default=0.5)
    s.add_argument("--value", type=float)
    s.add_argument("--use", action="store_true", help="route: make the match active")
    s.add_argument("--force", action="store_true")
    s.add_argument("--json", action="store_true")
    s.set_defaults(fn=cmd_persona)

    s = sub.add_parser("skill")
    s.add_argument("action", choices=["list", "new", "log", "snapshot", "commit",
                                      "rollback", "link"])
    s.add_argument("name", nargs="?", default="")
    s.add_argument("--description")
    s.add_argument("--persona")
    s.add_argument("--outcome", choices=["ok", "fail"], default="ok")
    s.add_argument("--note")
    s.add_argument("--to", type=int)
    s.add_argument("--target", default="~/.claude/skills")
    s.add_argument("--json", action="store_true")
    s.set_defaults(fn=cmd_skill)

    s = sub.add_parser("remind")
    s.add_argument("action", choices=["add", "list", "due", "done", "snooze", "notify"])
    s.add_argument("text", nargs="?", default="")
    s.add_argument("--at")
    s.add_argument("--in", dest="in_")
    s.add_argument("--every", help="daily, weekly, or 3d / 12h …")
    s.add_argument("--within", help="due: also what comes due within 2h, 1d …")
    s.add_argument("--persona")
    s.add_argument("--json", action="store_true")
    s.set_defaults(fn=cmd_remind)

    s = sub.add_parser("cabinet")
    s.add_argument("personas", nargs="*")
    s.add_argument("--question", "-q", default="")
    s.set_defaults(fn=cmd_cabinet)

    s = sub.add_parser("buddy")
    s.add_argument("--rename")
    s.add_argument("--json", action="store_true")
    s.set_defaults(fn=cmd_buddy)

    s = sub.add_parser("session")
    s.add_argument("action", choices=["search", "list", "show", "add", "forget", "prune"])
    s.add_argument("text", nargs="?", default="")
    s.add_argument("--session")
    s.add_argument("--role", default="user")
    s.add_argument("--around", type=int, default=3)
    s.add_argument("--older", help="prune: 90d, 26w …")
    s.add_argument("--private", action="store_true")
    s.add_argument("--include-private", action="store_true")
    s.add_argument("--limit", type=int, default=8)
    s.add_argument("--json", action="store_true")
    s.set_defaults(fn=cmd_session)

    s = sub.add_parser("hook")
    s.add_argument("event", choices=["session-start", "prompt", "stop"])
    s.set_defaults(fn=cmd_hook)
    return ap


def main(argv: list[str] | None = None) -> int:
    args = build_parser().parse_args(argv)
    if args.group in ("mem", "persona", "skill", "remind", "session") and args.action not in (
        "list", "recall", "stats", "check", "due", "notify", "prune"
    ):
        needs = getattr(args, "text", None) if args.group != "skill" else args.name
        if not needs and not (args.group == "persona" and args.action in ("new", "show")):
            raise SystemExit(f"{args.group} {args.action} needs an argument")
    root = home()
    store = Store(root)
    started = time.monotonic()
    try:
        args.fn(args, root, store)
    finally:
        store.conn.close()
        if os.environ.get("BASTION_PACK_TIMING"):
            print(f"[{(time.monotonic() - started) * 1000:.0f} ms]", file=sys.stderr)
    return 0


if __name__ == "__main__":
    sys.exit(main())
