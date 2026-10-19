---
name: bastion-reminders
description: Bastion's reminders and proactivity — turn anything with a time into a reminder, surface what's due, follow up on commitments, and send desktop notifications without a daemon. Use when the person mentions a deadline, appointment, routine, or "remind me", and at the start of every session.
license: MIT
metadata:
  version: "0.1.0"
  author: bastion
---

# Bastion reminders

`B` = `python3 <dir of the bastion skill>/scripts/bastion.py`.

## Creating

Whenever the person mentions something with a time — even in passing
("I have the dentist Thursday at 3") — offer once to remind them, or just
set it if they asked.

```bash
B remind add "Dentist" --at "2026-10-23 15:00"          # local time
B remind add "Send the invoice" --in 2h
B remind add "Weekly review" --at "2026-10-26 18:00" --every weekly
B remind add "Take the medicine" --at "2026-10-20 08:00" --every daily --persona health
```

`--every` takes `daily`, `weekly`, or `12h`, `3d`, `2w`. Set a reminder a
little before the event when preparation is needed (leave time to travel).

## Surfacing

- Session start: `B brief` lists what's due within 12 hours — mention it.
- `B remind due --within 1d` for "what do I have today?".
- `B remind list` for everything pending.
- Done: `B remind done <id>` (repeating ones move to the next time).
- Later: `B remind snooze <id> --in 1h`.

## Proactivity

Being proactive means noticing, not nagging. Once per session at most, and
only when it fits:

- A commitment the person made earlier (a `decision` or `event` memory)
  has a date that has passed: ask how it went, record the outcome.
- A persona whose area hasn't come up in a long time and has open goals:
  one line, e.g. "Health hasn't heard from you in two weeks — still on 3
  workouts a week?"
- The weekly review: if the person has a weekly reminder for it, run it as a
  Cabinet of their personas (skill `bastion-cabinet`).

## Notifications outside a session (optional)

No daemon: the OS scheduler runs one command. `B init` copies the CLI to
`~/.bastion/bin/bastion.py`, a path that survives updates. `B remind notify` sends a
desktop notification (notify-send on Linux, osascript on macOS) for each due
reminder not yet shown. Offer it to the person; don't install it
without asking:

```bash
# cron, every 5 minutes
*/5 * * * * python3 ~/.bastion/bin/bastion.py remind notify
```
