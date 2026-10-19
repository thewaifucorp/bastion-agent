---
name: bastion-memory
description: Bastion's durable memory — store and search facts, preferences, decisions, events and lessons about the person in a local SQLite database with full-text search and a small knowledge graph, and search past conversations. Use whenever the person tells you something worth knowing next time, before answering anything personal, and when they ask "what did we discuss about X".
license: MIT
metadata:
  version: "0.1.0"
  author: bastion
---

# Bastion memory

`B` = `python3 <dir of the bastion skill>/scripts/bastion.py`.

One file, `~/.bastion/memory.db`: memories with full-text search (SQLite
FTS5, BM25 ranking nudged by importance and recency) and a knowledge graph
of entities and relations. Forgetting is soft: a forgotten memory stops
being returned but its history stays.

## When to store

Store when the person states something that will still matter next week:

| kind | example |
|---|---|
| `preference` | "prefers short answers", "no meetings before 10" |
| `fact` | "daughter Ana, 7", "works at a bank as a data engineer" |
| `decision` | "chose Postgres over Mongo for the app, because…" |
| `event` | "moved to Lisbon in March 2026" |
| `lesson` | "the deploy fails if the migration runs first" |
| `note` | anything else |

Don't store small talk, things only true for this conversation, or secrets.
One fact per memory, written so it stands alone without the conversation.

```bash
B mem add "Prefers short answers, no preamble" --kind preference --importance 5
B mem add "Knee injury, avoids running" --kind fact --persona health
B mem add "Ana is Sam's daughter" --entities "Ana:person,Sam:person" \
  --relation "Ana daughter_of Sam"
```

- `--importance` 1–5 (5 = always relevant; shown at every session start).
- `--persona` files it under one area; omit it for things about the person
  as a whole. A memory under a `local-only` persona is private automatically;
  `--private` forces it.

## When to search

Before asking the person anything, and before advice that depends on them:

```bash
B mem search "running knee"          # top 5, private hidden
B mem search "budget" --persona finance --limit 10
B mem recall                          # most important + recent
B mem graph Ana                       # what's linked to an entity
```

Use what you find without announcing "according to my memory" every time.

## Past conversations

Memories are what you chose to keep. Conversations are everything that was
said: with the Claude Code plugin, a hook stores each turn (the person's
prompts and your replies; not tool calls, tool output or thinking). Search
them when the person says "we talked about this", or when memory has
nothing and the answer may be in an old session:

```bash
B session search "postgres password"   # hits as <session>#<turn>
B session show 3f9a2c1e#14             # the turns around a hit
B session list                         # recent sessions, first prompt each
```

Turns under a `local-only` persona are private, like memories. Hosts
without hooks can log turns with `B session add "<text>" --session <id>
--role user|assistant`. `B session forget <id>` deletes a session;
`B session prune --older 180d` deletes old turns. Something from a past
conversation that will matter again: store it as a memory.

## Keeping it right

- The person corrects something: `B mem forget <id>`, then add the right one.
- A memory turns out wrong or stale: same.
- `B mem stats` shows the size.
