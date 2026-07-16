# Pet pack schema v1

## Contract

- `schema`: must be `1`.
- `id`: lowercase `publisher/name` using letters, digits, and hyphens.
- `name`: 1–32 characters.
- `rarity`: `common`, `uncommon`, `rare`, `epic`, or `legendary`.
- `palette.primary` and `palette.secondary`: optional `#RRGGBB` colors.
- `states`: all seven states are required.
- `interval_ms`: 90–10000.
- `frames`: 1–16 frames; each frame has 1–6 lines, at most 21 display columns.

## Color markup

Use `{primary}`, `{secondary}`, `{cyan}`, `{blue}`, `{magenta}`, `{amber}`,
`{green}`, `{red}`, `{muted}`, and `{reset}` inside frame strings. Markup does
not consume terminal width. Unknown tags and control/ANSI characters are
rejected.

## State semantics

- `onboarding`: first contact and setup.
- `guard`: normal idle state.
- `thinking`: general request in progress.
- `build`: coding, debugging, tests, or file work.
- `cabinet`: deliberation and reprioritization.
- `success`: short confirmation animation.
- `alert`: failure or attention required.

Packs are data-only UI assets. Speech, game rules, hooks, tools, and external
side effects are outside this schema.
