# Personas and Cabinet

Personas give one Bastion instance distinct, reviewable perspectives for the different domains of a life: work, health, relationships, learning, finances, or a specific project. They are not separate bots and they do not bypass the runtime’s identity, capability, or privacy boundaries.

In the Compose deployment, the repository’s `personas/` directory is mounted read-only into the core container. Treat persona files as policy: review changes, keep secrets out of them, and do not allow untrusted conversation content to rewrite them.

## Persona contract

A persona is a `SOUL.md` file: YAML frontmatter followed by the system prompt. The frontmatter fields:

| Field | Required | Meaning |
|---|---|---|
| `name` | yes | Persona identifier, referenced by `/cabinet` and pack manifests. |
| `description` | yes | One-line summary shown in listings. |
| `bastion.privacy_tier` | yes | `local-only` or `cloud-ok` — gates which providers this persona may route through. |
| `bastion.weight` | yes | Relative influence in Cabinet deliberation. |
| `skills` | no | Skill names this persona can invoke. |
| `objectives` | yes | What this persona exists to accomplish — a short list of outcome statements. |
| `goals` | yes | Concrete, checkable definitions of success for this persona's output. |
| `scope` | yes | One sentence stating what this persona explicitly does *not* do — the boundary, not the mission. |
| `tools` | no | Capability allowlist. Omitted/`null` = unrestricted (this persona can invoke any capability the runtime exposes) — this is the only clean way to declare "no tool restriction." A populated list = exactly those capabilities, nothing else. An explicit `tools: []` parses but is flagged as a likely mistake (see below), not a clean way to say "no tools." |

`objectives`, `goals`, and `scope` are required for any persona written against
the current contract, but existing personas written before this contract
(like the bundled `default` persona, which has none of these fields) keep
working unchanged — the fields are additive and optional at the parser
level; a missing-but-required field shows up as a *validation* problem, not
a parse failure. A persona that fails validation is loaded with the problems
attached (`problems: Vec<String>`, never a hard error), so an operator
editing personas through the web UI or `/proposal` sees exactly what's
missing before publishing, rather than a silent skip or a 500.

`validate()` treats `tools: []` (an explicit, empty allowlist) as a
*problem*, not a clean statement of "no tools": an author who writes it out
almost always meant to list something and forgot, and once resolved into
`allowed_tools` it silently denies every single tool call — a confusing
failure mode to debug from the outside. If you genuinely want a
tools-free, planning-only persona, omit the key entirely rather than writing
`tools: []`; the *absence* of the field is what actually means unrestricted,
so there is currently no way to spell "restricted to nothing" without
tripping this warning. (The warning doesn't block loading or Cabinet use —
`validate()` never hard-fails a load, only an apply-path caller like
`/proposal approve` surfaces it — so a pack can ship with `tools: []` and
still work; it just carries a visible "problem" an operator will see.)

When a persona *does* declare a populated `tools` list, every turn it drives
resolves that list into the turn's `allowed_tools`, and the capability
registry rejects any tool call outside it — checked first, before egress or
approval policy, so a persona can't reach a capability its own contract
didn't name even if some other policy would otherwise allow it.

A real example, from the `software-sdlc` pack's `implementer` persona —
restricted to exactly one capability:

```yaml
---
name: implementer
description: Executes an approved implementation plan — writes the code, runs the tests, commits in small steps.
bastion:
  privacy_tier: cloud-ok
  weight: 0.8
skills:
  - sdlc-implement
objectives:
  - "Execute an approved implementation plan: write the code, run the tests, commit progressively"
goals:
  - "Every commit builds and passes tests on its own where practical"
tools:
  - git
scope: "Local workspace only — git capability limited to init/status/diff/add/commit/branch/log; no push/remote/merge"
---
```

Any tool call this persona attempts outside `git` is rejected before it
reaches the capability registry's other policies. The pack's `tech-lead`
persona, by contrast, ships with `tools: []` (deliberately tool-free,
planning-only) — a real, currently-shipping example of the "flagged but not
blocked" case described above.

## Cabinet

The console command below convenes named personas for the next eligible Cabinet deliberation:

```text
/cabinet <persona1> [persona2 ...]
```

Cabinet is for trade-offs, not fake consensus. It can preserve dissent while producing a synthesized recommendation, making it useful when competing priorities need to be made explicit and reconsidered.

Examples:

```text
/cabinet career health finance
/cabinet project-owner tech-lead
```

Use personas to give context a durable home. Use Cabinet when those contexts should disagree before you decide.
