# `uia` — Windows UI Automation helper

The desktop helper a Bastion **node** uses to run the `ui.snapshot` and `ui.act`
capabilities (spec `multi-device-brain-and-nodes.md`, BMD-13/BMD-14).

## Why it is a separate package

`bastion-agent` sets `unsafe_code = "forbid"`. UI Automation needs COM / Win32
(`unsafe`), so it cannot live in the agent crate. This is a **standalone**
package (its own workspace, outside `bastion-agent`'s build): the agent's
`ui.snapshot` / `ui.act` node capabilities only **orchestrate** it, talking to
it over JSON on stdin/stdout. The agent never links any UI Automation code.

## Protocol

One JSON command per line on **stdin**; one JSON reply per line on **stdout**.

Commands (`cmd`):

- `resolve` — find a top-level window without capturing or sending input, and
  report its owning process, so the agent can check the grant's app scope
  **before** anything is captured (BMD-13).
  ```json
  {"cmd":"resolve","match":{"exe_name":"blender.exe"}}
  {"cmd":"resolve","match":{"title_contains":"Blender"}}
  {"cmd":"resolve","match":{"foreground":true}}
  ```
- `capture` — re-verify the window still belongs to that process, then return
  its UI Automation tree and a PNG screenshot.
  ```json
  {"cmd":"capture","hwnd":123456,"pid":42,"exe":"C:\\apps\\blender.exe"}
  ```
- `act` — re-verify the window still belongs to that process
  (`{"ok":false,"error":"window_gone"}` otherwise, never acting on another
  window), then perform the action.
  ```json
  {"cmd":"act","hwnd":123456,"pid":42,"exe":"C:\\apps\\blender.exe",
   "action":{"kind":"invoke","target":{"name":"Render"}}}
  {"cmd":"act", "...":"...",
   "action":{"kind":"set_value","target":{"automation_id":"fileName"},"value":"scene.blend"}}
  ```

Replies are `{"ok":true, ...}` on success, or
`{"ok":false,"error":"window_gone"|"cancelled"|"failed","detail":"..."}`.

The agent maps `window_gone` → `InvokeError::WindowGone` and everything else to a
typed error; scope, snapshot freshness and the required `intent` are all
enforced on the agent side (`src/devices/ui.rs`), never here.

## Where the agent looks for it

`SubprocessBackend::locate()` resolves, in order:

1. `$BASTION_UIA_BIN` — an explicit path.
2. `uia.exe` next to the running `bastion` executable.

Ship `uia.exe` beside `bastion.exe` in the Windows installer, or set
`BASTION_UIA_BIN`.

## Build

Windows only:

```powershell
cargo build --release            # on Windows
```

The crate compiles to a stub on non-Windows hosts (every command returns an
"unsupported" error) so the workspace stays inspectable. To type-check the real
Windows code from another OS without a linker:

```sh
rustup target add x86_64-pc-windows-gnu
cargo clippy --target x86_64-pc-windows-gnu
```

## Anti-goals (from the spec §7/§8)

- No pixel-vision first: the structured UI Automation tree comes first; a
  screenshot is evidence, not the control channel.
- No app-specific logic (Blender, Office, …): those arrive as capability packs
  built on these primitives.
- No listening port: the helper only reads stdin and writes stdout.
- Nothing outside the granted apps: the agent refuses any window whose owning
  executable is not in the grant, before this helper is ever asked to act.
