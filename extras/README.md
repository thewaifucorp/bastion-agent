# Extras

Things that ship with Bastion but are not Bastion's runtime and do not talk
to its daemon. Each one stands alone, with its own README, tests and CI job.

| Extra | What it is |
|---|---|
| [bastionizer](bastionizer/README.md) | Skills that make any agent act as Bastion with no daemon: personas, local memory, conversations, reminders, self-written skills, Cabinet, companion. An emulation, not an integration. |

Not here:
- `integrations/` — how another agent or platform connects to a running
  Bastion (the Claude Code plugin, the Paperclip adapter).
- [bastion-extensions](https://github.com/thewaifucorp/bastion-extensions) —
  signed packs installed *into* a Bastion daemon.
