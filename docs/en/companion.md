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

Inside the TUI, `/theme <rgb|cyan|blue|magenta|amber|green|mono>` switches the
theme instantly and `/theme #RRGGBB` sets a custom accent. The choice persists
in `~/.config/bastion/tui.json` (precedence: `bastion.toml` < `tui.json` <
environment variables).

## Native mascot

The native family is now the pixel-art Keeper (steel helmet, dark visor, face
that changes per state) and Patchwork, which takes over when the Cabinet is
convened. Extra local states: an unknown slash command shows the Keeper in
doubt (open eyes and an amber question mark) and `/pet sleep` shows the
resting face with zzz.

On terminals with a graphics protocol (Kitty, WezTerm, Ghostty, foot, Konsole,
iTerm2, and VS Code with `terminal.integrated.enableImages`), the mascot is
drawn as a real image — pixel art identical to the README mark, small and
crisp, with the state seal (shield, ?, !, zzz…) in a fixed slot beside the
head. Without a protocol (Zed's built-in terminal, stock GNOME Terminal) it
falls back to a minimal glyph face — eyes and seal only, crisp in any font.
`BASTION_TUI_GRAPHICS=off` forces text mode.

## Care and progression

Game mode rewards completed turns, never token volume. Build and Cabinet turns
earn slightly more cosmetic XP, but levels never grant tools, permissions, or
policy bypasses. Human input also fills a live momentum meter: every 80
characters becomes 1 XP when sent, capped at 3 XP per message. A completed AI
reply contributes a fixed 1 XP regardless of its token or character count.

Inside the TUI:

```text
/pet stats
/pet game on
/pet feed apple
/pet water
/pet play puzzle
/pet sleep nap
/pet use path/to/pet.toml
```

Type a trailing space after `/pet feed`, `/pet play`, or `/pet sleep` to open
their emoji-assisted choice menus. The pantry has eight foods, play has eight
activities, and rest has four durations; each choice restores a different mix
of the water, food, play, and rest meters shown by `/pet stats`.

Needs advance only during observed active time; idle gaps longer than five
minutes count as breaks. Water also doubles as a gentle reminder for the
developer. The companion never dies, loses XP, or shames the user.

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
