# Bastionizer

Skills that make any agent act as Bastion, with **no daemon**: personas for
the areas of your life, a local memory, reminders, skills the agent writes
and improves itself, Cabinet deliberation, and a status-line companion.

Everything stays on your machine in `~/.bastion` (`$BASTION_HOME`). The only
code is one Python 3.9+ script using the standard library (SQLite with FTS5
is part of it). No server, no background process, no network.

This is different from the [Claude Code plugin](../claude-code/), which
connects Claude Code to a running Bastion daemon (approvals from your
phone, the desk device, shared memory). Use the Bastionizer when you want the
Bastion way of working without running Bastion; use the plugin when you
run Bastion.
Install one or the other: when the daemon's `bastion` plugin is enabled, the
Bastionizer's hooks stay quiet (`BASTIONIZER_FORCE=1` overrides).

## What's inside

| Skill | What the agent gets |
|---|---|
| `bastion` | Who it is and the loop: route → recall → act → remember → learn. Session brief. |
| `bastion-personas` | Create, route, switch and weigh personas (`SOUL.md`, same contract as Bastion). |
| `bastion-memory` | Memories in SQLite + FTS5 (BM25, importance, recency), a knowledge graph, soft forgetting; full-text search over past conversations. |
| `bastion-skill-forge` | Writes a skill after a task that worked; logs outcomes; patches and versions skills that failed. |
| `bastion-reminders` | Reminders, repeats, snooze, proactive follow-ups, optional desktop notifications. |
| `bastion-cabinet` | Personas deliberate on a trade-off, dissent kept, weighted recommendation. |

```
~/.bastion/
├── memory.db              memories, graph, conversations, reminders, skill outcomes, weights, companion
├── personas/<slug>/SOUL.md
├── skills/<name>/SKILL.md  skills the agent wrote (agentskills format)
└── bin/bastion.py         stable copy of the CLI, for a status line or cron
```

## Install

### Claude Code (plugin: skills + hooks + commands)

```bash
claude plugin marketplace add /path/to/bastion-agent/extras/bastionizer
claude plugin install bastionizer@bastionizer
```

The plugin's hooks put the session brief in context at start and, on every
prompt, route to a persona and add matching memories and due reminders
(about 50 ms, at most 1.5 KB); after each reply, an async hook stores what
was said (prompts and replies, not tool output) for `session search`. Commands: `/bastionizer:persona`,
`/bastionizer:remember`, `/bastionizer:remind`, `/bastionizer:cabinet`.

Companion in the status line (`~/.claude/settings.json`), after the first
session has run:

```json
{ "statusLine": { "type": "command", "command": "python3 ~/.bastion/bin/bastion.py buddy" } }
```

### Any other agent (skills only)

Copy the six folders under `skills/` into the agent's skills directory, keeping
them side by side (they find the CLI at `../bastion/scripts/bastion.py`):

```bash
cp -r extras/bastionizer/skills/* ~/.codex/skills/        # Codex
cp -r extras/bastionizer/skills/* ~/.claude/skills/       # Claude Code without the plugin
```

Agents without a skills directory: put the `bastion` skill's text in the
agent's instructions file (`AGENTS.md` and the like). Without hooks the
agent runs `bastion.py brief` itself at the start of a session, as the
skill tells it to.

## Try it

```bash
B="python3 extras/bastionizer/skills/bastion/scripts/bastion.py"
$B init --name Bit
$B persona new --name Work --description "Code and career" --keywords "code,deploy,meeting" \
  --objectives "ship" --goals "tests pass" --scope "no money talk"
$B mem add "Prefers short answers" --kind preference --importance 5
$B remind add "Weekly review" --in 2h --every weekly
$B brief
$B buddy
```

Tests: `python3 -m unittest discover -s extras/bastionizer/tests`.

## Privacy

Personas are `cloud-ok` or `local-only`. Memories filed under a `local-only`
persona are private: hooks never inject them and search hides them unless
asked. The Bastionizer can't choose which model the host agent uses; if the host
runs a cloud model, what the agent reads goes to it. Keep secrets out of
memory, personas and skills.

## Limits

- No notifications while no agent is running, unless you add the optional
  cron line (`bastion-reminders`).
- Routing is keyword-based: tune a persona's `trigger_keywords` when it
  picks the wrong one.
- Memory search is lexical (FTS5), not semantic. A Bastion install has
  semantic memory; the Bastionizer keeps zero dependencies.
- On Windows, call `python` or `py` instead of `python3` in the hooks and
  status line.
