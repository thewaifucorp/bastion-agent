# Terminal companion

Bastion's TUI treats the mascot as a visible runtime state, not a decorative
sticker. The built-in family uses Terminal Familiar during onboarding, Keeper
for normal guard/thinking states, and Patchwork for build and Cabinet work.
The fixed identity panel collapses automatically on small terminals.

## Theme

Configure the terminal in `bastion.toml`:

```toml
[tui]
theme = "rgb" # rgb, cyan, blue, magenta, amber, green, mono
# accent = "#68d9ff"
mascot = true
animations = true
game = false
# pet = "extensions/acme/keeper/ui/pet.toml"
```

`BASTION_TUI_THEME`, `BASTION_TUI_ACCENT`, `BASTION_TUI_MASCOT`,
`BASTION_TUI_ANIMATIONS`, `BASTION_TUI_GAME`, and `BASTION_TUI_PET` override
those TUI-only values. `NO_COLOR` disables the RGB palette.

## Care and progression

Game mode rewards completed turns, never token volume. Build and Cabinet turns
earn slightly more cosmetic XP, but levels never grant tools, permissions, or
policy bypasses.

Inside the TUI:

```text
/pet stats
/pet game on
/pet feed
/pet water
/pet play
/pet sleep
/pet use path/to/pet.toml
```

Feed, play, and sleep are the companion's care loop; water doubles as a gentle
reminder for the developer. Needs advance only during observed active
time; idle gaps longer than five minutes count as breaks. The companion never
dies, loses XP, or shames the user. `play` may mean stretching, walking,
breathing, a short check-in, or any reset that works—not only Pomodoro.

## External coding-agent sessions

Claude, Codex, OpenCode, and other integrations can report generic lifecycle
events without depending on their binaries:

```bash
bastion companion event session-start --source codex
bastion companion event activity --source codex
bastion companion event session-stop --source codex
```

The MCP server also exposes the local `bastion_companion_event` tool with
`event` and `source` arguments. Hooks/plugins still need to call one of these
bridges; Bastion does not pretend it can observe an unrelated process without
an integration.

## Custom pet packs

A pet is a strict, data-only TOML asset with seven required animated states:
`onboarding`, `guard`, `thinking`, `build`, `cabinet`, `success`, and `alert`.
Frames are capped at 21 columns by 6 rows, control/ANSI characters are rejected,
and the file cannot execute code or request authority.

Use the bundled `bastion-pet-pack` skill to scaffold and validate a pack. An
extension can ship the resulting `ui/pet.toml` as `Provided::Ui("pet")` with an
empty permission set.
