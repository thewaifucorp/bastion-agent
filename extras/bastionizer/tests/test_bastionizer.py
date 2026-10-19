"""Bastionizer CLI tests. Standard library only:

    python3 -m unittest discover -s extras/bastionizer/tests
"""

from __future__ import annotations

import contextlib
import io
import json
import os
import sqlite3
import sys
import tempfile
import unittest
from datetime import timedelta
from pathlib import Path

SCRIPTS = Path(__file__).resolve().parents[1] / "skills" / "bastion" / "scripts"
sys.path.insert(0, str(SCRIPTS))
import bastion  # noqa: E402


class BastionizerTest(unittest.TestCase):
    def setUp(self) -> None:
        self.tmp = tempfile.TemporaryDirectory()
        os.environ["BASTION_HOME"] = self.tmp.name
        self.home = os.environ.get("HOME")
        os.environ["HOME"] = self.tmp.name  # no real ~/.claude settings leak in
        os.environ.pop("BASTION_SKILLS_DIR", None)

    def tearDown(self) -> None:
        if self.home is not None:
            os.environ["HOME"] = self.home
        self.tmp.cleanup()

    def run_cli(self, *argv: str, stdin: str | None = None) -> str:
        out = io.StringIO()
        old_stdin = sys.stdin
        if stdin is not None:
            sys.stdin = io.StringIO(stdin)
        try:
            with contextlib.redirect_stdout(out):
                bastion.main(list(argv))
        finally:
            sys.stdin = old_stdin
        return out.getvalue()

    def make_personas(self) -> None:
        self.run_cli("persona", "new", "--name", "Work", "--description", "Code and career",
                     "--keywords", "code,deploy,meeting", "--weight", "0.7",
                     "--objectives", "ship", "--goals", "tests pass", "--scope", "no money")
        self.run_cli("persona", "new", "--name", "Saúde", "--description", "Health",
                     "--keywords", "sleep,slept,gym,dor", "--tier", "local-only",
                     "--objectives", "rest", "--goals", "7h", "--scope", "no diagnosis")

    # -------------------------------------------------------- frontmatter

    def test_frontmatter_round_trip_and_block_styles(self) -> None:
        meta = {"name": "x", "bastion": {"privacy_tier": "cloud-ok", "weight": 0.4},
                "trigger_keywords": ["a", "b"], "scope": 'say "no"'}
        parsed, body = bastion.parse_frontmatter(bastion.dump_frontmatter(meta) + "\nbody")
        self.assertEqual(parsed, meta)
        self.assertEqual(body, "body")
        doc = (
            "---\nname: y\ndescription: >\n  folded\n  text\nskills:\n  - one\n  - two\n"
            "bastion:\n  weight: 0.9\n  tools:\n    - git\nscope: plain words\n---\nhi\n"
        )
        parsed, _ = bastion.parse_frontmatter(doc)
        self.assertEqual(parsed["description"], "folded text")
        self.assertEqual(parsed["skills"], ["one", "two"])
        self.assertEqual(parsed["bastion"], {"weight": 0.9, "tools": ["git"]})
        self.assertEqual(parsed["scope"], "plain words")

    # ------------------------------------------------------------ personas

    def test_routing_picks_by_keyword_folds_accents_and_sticks(self) -> None:
        self.make_personas()
        self.assertIn("work", self.run_cli("persona", "route", "the deploy broke"))
        self.assertIn("saude", self.run_cli("persona", "route", "DÓR nas costas", "--use"))
        out = json.loads(self.run_cli("persona", "route", "what's for lunch", "--json"))
        self.assertEqual(out["persona"], "saude")  # no match keeps the active one
        self.assertEqual(self.run_cli("persona", "check").strip(), "2 personas ok")

    def test_weight_changes_are_kept_with_reasons(self) -> None:
        self.make_personas()
        self.run_cli("persona", "weight", "work", "--value", "0.9", "--why", "launch week")
        listed = json.loads(self.run_cli("persona", "list", "--json"))
        work = next(p for p in listed if p["slug"] == "work")
        self.assertEqual((work["base_weight"], work["weight"]), (0.7, 0.9))
        self.assertIn("launch week", self.run_cli("persona", "history", "work"))

    # -------------------------------------------------------------- memory

    def test_memory_search_ranks_hides_private_and_forgets(self) -> None:
        self.make_personas()
        mid = self.run_cli("mem", "add", "Prefers short answers", "--kind", "preference",
                           "--importance", "5").split()[-1]
        self.run_cli("mem", "add", "Back pain since monday", "--persona", "saude")
        hits = json.loads(self.run_cli("mem", "search", "short answers", "--json"))
        self.assertEqual([h["id"] for h in hits], [mid])
        self.assertEqual(self.run_cli("mem", "search", "back pain").strip(), "nothing found")
        self.assertIn("Back pain", self.run_cli("mem", "search", "back pain", "--include-private"))
        self.run_cli("mem", "forget", mid)
        self.assertEqual(self.run_cli("mem", "search", "short answers").strip(), "nothing found")

    def test_search_input_cannot_inject_fts_syntax(self) -> None:
        self.run_cli("mem", "add", "alpha beta")
        for query in ['" OR 1=1 --', "NEAR(alpha", "alpha*", "a AND NOT b", ""]:
            self.run_cli("mem", "search", query or "-")  # must not raise

    def test_graph_links_entities_and_forget_invalidates_them(self) -> None:
        mid = self.run_cli("mem", "add", "Ana is Sam's daughter", "--entities",
                           "Ana:person,Sam:person", "--relation", "Ana daughter_of Sam").split()[-1]
        self.assertIn("Ana —daughter_of→ Sam", self.run_cli("mem", "graph", "Ana"))
        self.run_cli("mem", "forget", mid)
        self.assertIn("no entity", self.run_cli("mem", "graph", "Ana"))

    # ----------------------------------------------------------- reminders

    def test_reminders_due_repeat_and_snooze(self) -> None:
        rid = self.run_cli("remind", "add", "water", "--in", "0m", "--every", "daily").split()[0]
        later = self.run_cli("remind", "add", "dentist", "--in", "3d").split()[0]
        due = self.run_cli("remind", "due")
        self.assertIn(rid, due)
        self.assertNotIn(later, due)
        self.assertIn("next", self.run_cli("remind", "done", rid))
        self.assertEqual(self.run_cli("remind", "due").strip(), "nothing pending")
        self.assertIn(later, self.run_cli("remind", "due", "--within", "4d"))
        self.run_cli("remind", "snooze", later, "--in", "1w")
        self.assertNotIn(later, self.run_cli("remind", "due", "--within", "4d"))

    def test_importance_takes_a_fraction_too(self) -> None:
        self.assertEqual([bastion.importance(v) for v in ("0.7", "5", "9", "0")], [4, 5, 5, 1])

    def test_bad_duration_is_refused(self) -> None:
        with self.assertRaises(SystemExit):
            self.run_cli("remind", "add", "x", "--in", "soon")

    # -------------------------------------------------------------- skills

    def test_skill_lifecycle_flags_open_failures_until_improved(self) -> None:
        self.run_cli("skill", "new", "Release Flow", "--description", "Cut a release")
        self.run_cli("skill", "log", "release-flow", "--outcome", "fail", "--note", "no tag")
        self.assertIn("release-flow (no tag)", self.run_cli("brief"))
        self.run_cli("skill", "snapshot", "release-flow")
        path = Path(self.tmp.name) / "skills" / "release-flow" / "SKILL.md"
        path.write_text(path.read_text().replace("1. …", "1. tag it"))
        self.assertIn("v2", self.run_cli("skill", "commit", "release-flow", "--note", "tag"))
        self.assertNotIn("failures to improve", self.run_cli("brief"))
        self.run_cli("skill", "rollback", "release-flow")
        self.assertIn("1. …", path.read_text())
        meta, _ = bastion.parse_frontmatter(path.read_text())
        self.assertEqual(meta["name"], "release-flow")

    def test_skill_names_stay_inside_the_skills_dir(self) -> None:
        self.run_cli("skill", "new", "../../etc/evil", "--description", "x")
        self.assertTrue((Path(self.tmp.name) / "skills" / "etc-evil" / "SKILL.md").exists())

    # ---------------------------------------------------- cabinet & hooks

    def test_cabinet_shares_the_vote_by_weight(self) -> None:
        self.make_personas()
        out = self.run_cli("cabinet", "work", "saude", "-q", "work tonight?")
        self.assertIn("58% of the vote", out)
        self.assertIn("42% of the vote", out)

    def test_prompt_hook_routes_recalls_and_shows_due_once(self) -> None:
        self.make_personas()
        self.run_cli("mem", "add", "deploy needs the VPN on", "--kind", "lesson")
        self.run_cli("mem", "add", "sleep apnea", "--persona", "saude")
        self.run_cli("remind", "add", "stand up", "--in", "0m")
        self.run_cli("persona", "use", "saude")
        out = self.run_cli("hook", "prompt", stdin=json.dumps({"prompt": "deploy the code"}))
        self.assertIn("switch to persona work", out)
        self.assertIn("VPN", out)
        self.assertIn("stand up", out)
        again = self.run_cli("hook", "prompt", stdin=json.dumps({"prompt": "sleep"}))
        self.assertNotIn("stand up", again)  # shown once per 30 minutes
        self.assertNotIn("apnea", again)  # private memories never reach hooks
        self.assertLessEqual(len(out), bastion.HOOK_BUDGET + 1)

    def test_session_start_brief_installs_a_stable_cli(self) -> None:
        self.make_personas()
        self.run_cli("mem", "add", "Lives in Lisbon", "--importance", "5")
        out = self.run_cli("hook", "session-start", stdin="{}")
        self.assertIn("* work", out)
        self.assertIn("Lives in Lisbon", out)
        self.assertTrue((Path(self.tmp.name) / "bin" / "bastion.py").exists())

    def test_stop_hook_keeps_what_was_said_for_session_search(self) -> None:
        self.make_personas()
        transcript = Path(self.tmp.name) / "t.jsonl"
        soon = bastion.iso(bastion.now() + timedelta(minutes=1))
        lines = [
            {"type": "user", "uuid": "u1", "timestamp": soon,
             "message": {"role": "user", "content": "how do I rotate the postgres password?"}},
            {"type": "assistant", "uuid": "a1", "timestamp": soon,
             "message": {"content": [{"type": "thinking", "thinking": "secret plan"},
                                     {"type": "text", "text": "Use ALTER ROLE then reload."},
                                     {"type": "tool_use", "name": "Bash", "input": {}}]}},
            {"type": "user", "uuid": "r1", "message": {"content": [
                {"type": "tool_result", "content": "tool output"}]}},
            {"type": "user", "uuid": "m1", "isMeta": True, "message": {"content": "meta"}},
            {"type": "assistant", "uuid": "s1", "isSidechain": True,
             "message": {"content": [{"type": "text", "text": "subagent chatter"}]}},
            {"type": "user", "uuid": "c1",
             "message": {"content": "<command-name>/clear</command-name>"}},
        ]
        transcript.write_text("".join(json.dumps(x) + "\n" for x in lines) + '{"type": "us')
        event = json.dumps({"session_id": "sess-1234abcd", "transcript_path": str(transcript)})
        self.run_cli("hook", "stop", stdin=event)
        self.run_cli("hook", "stop", stdin=event)  # same transcript again: no duplicates
        hits = json.loads(self.run_cli("session", "search", "postgres password", "--json"))
        self.assertEqual([h["id"] for h in hits], ["u1"])
        shown = self.run_cli("session", "show", "sess-123#1")
        self.assertIn("ALTER ROLE", shown)
        for absent in ("secret plan", "tool output", "meta", "subagent", "/clear"):
            self.assertNotIn(absent, shown)
        self.assertIn("2 turns", self.run_cli("session", "list"))
        # The half-written last line is read once it is complete.
        with transcript.open("a") as f:
            f.write(f'er", "uuid": "u2", "timestamp": "{soon}", '
                    '"message": {"content": "and mysql?"}}\n')
        self.run_cli("hook", "stop", stdin=event)
        self.assertIn("mysql", self.run_cli("session", "search", "mysql"))

    def test_session_turns_under_a_local_only_persona_are_private(self) -> None:
        self.make_personas()
        self.run_cli("persona", "use", "saude")
        self.run_cli("session", "add", "my knee hurts", "--session", "s1", "--role", "user")
        self.assertEqual(self.run_cli("session", "list").strip(), "no sessions yet")
        self.assertEqual(self.run_cli("session", "search", "knee").strip(), "nothing found")
        self.assertIn("knee", self.run_cli("session", "search", "knee", "--include-private"))
        self.assertIn("1 turns deleted", self.run_cli("session", "forget", "s1"))

    def test_stop_hook_attributes_turns_by_time_and_fails_closed(self) -> None:
        self.make_personas()  # work becomes active; backdate that to 2001
        db = sqlite3.connect(Path(self.tmp.name) / "memory.db")
        db.execute("UPDATE persona_changes SET at = '2001-01-01T00:00:00+00:00'")
        db.commit()
        db.close()
        transcript = Path(self.tmp.name) / "t.jsonl"
        lines = [
            {"type": "user", "uuid": "w", "timestamp": "2005-01-01T00:00:00Z",
             "message": {"content": "deploy notes alpha"}},
            {"type": "user", "uuid": "old", "timestamp": "2000-01-01T00:00:00Z",
             "message": {"content": "before any persona alpha"}},
            {"type": "user", "uuid": "none", "message": {"content": "no time alpha"}},
        ]
        transcript.write_text("".join(json.dumps(x) + "\n" for x in lines))
        # Switching to the local-only persona before the hook runs must not
        # make the earlier work turn private, nor the unknown ones public.
        self.run_cli("persona", "use", "saude")
        self.run_cli("hook", "stop", stdin=json.dumps(
            {"session_id": "s", "transcript_path": str(transcript)}))
        hits = json.loads(self.run_cli("session", "search", "alpha", "--json"))
        self.assertEqual([h["id"] for h in hits], ["w"])
        self.assertEqual(hits[0]["persona"], "work")
        listed = self.run_cli("session", "list")
        self.assertIn("deploy notes", listed)
        self.assertNotIn("before any persona", listed)

    def test_hooks_step_aside_when_the_daemon_plugin_is_on(self) -> None:
        self.make_personas()
        project = Path(self.tmp.name) / "proj"
        (project / ".claude").mkdir(parents=True)
        (project / ".claude" / "settings.json").write_text(
            json.dumps({"enabledPlugins": {"bastion@bastion": True}}))
        event = {"cwd": str(project), "prompt": "deploy the code"}
        out = self.run_cli("hook", "prompt", stdin=json.dumps(event))
        self.assertEqual(out, "")
        self.assertIn("stays quiet", self.run_cli("hook", "session-start", stdin=json.dumps(event)))
        (project / ".claude" / "settings.local.json").write_text(
            json.dumps({"enabledPlugins": {"bastion@bastion": False}}))
        event["prompt"] = "slept badly"
        self.assertIn("saude", self.run_cli("hook", "prompt", stdin=json.dumps(event)))

    def test_buddy_worries_about_overdue(self) -> None:
        self.run_cli("init", "--name", "Bit")
        self.assertTrue(self.run_cli("buddy").startswith("(^‿^) Bit L1"))
        self.run_cli("remind", "add", "x", "--in", "0m")
        state = json.loads(self.run_cli("buddy", "--json"))
        self.assertEqual((state["mood"], state["overdue"]), ("worried", 1))

    def test_everything_is_one_sqlite_file(self) -> None:
        self.run_cli("mem", "add", "x", "--entities", "A:thing")
        tables = {r[0] for r in sqlite3.connect(Path(self.tmp.name) / "memory.db").execute(
            "SELECT name FROM sqlite_master WHERE type='table'")}
        self.assertTrue({"memories", "reminders", "entities", "relations"} <= tables)

    @unittest.skipIf(os.name != "posix", "POSIX permissions")
    def test_home_and_database_are_private(self) -> None:
        home = Path(self.tmp.name)
        home.chmod(0o755)
        self.run_cli("mem", "add", "x")
        self.assertEqual(home.stat().st_mode & 0o777, 0o700)
        self.assertEqual((home / "memory.db").stat().st_mode & 0o777, 0o600)
        self.run_cli("persona", "new", "--name", "Work", "--keywords", "code")
        soul = home / "personas" / "work" / "SOUL.md"
        self.assertEqual(soul.stat().st_mode & 0o077, 0)

    def test_naive_times_are_local(self) -> None:
        dt = bastion.parse_iso("2030-01-01 10:00")
        self.assertIsNotNone(dt.tzinfo)
        self.assertEqual(dt.utcoffset(), timedelta(0))


if __name__ == "__main__":
    unittest.main()
