# bastion-shell — the Bastion desktop shell

Slice 3 of the multi-device spec (`multi-device-brain-and-nodes.md`): the
casca desktop. BMD-15, BMD-27, BMD-28.

It is a tray icon plus one window that embeds the primary's web app (`/app`)
in a WebView2 — **no chat or memory UI of its own** (BMD-27). The tray has:

- **Open Bastion** — show/focus the window;
- **Stop node** — cut the local node now: kill the supervised `bastion node
  run`, or run the configured `stop_command` (BMD-15);
- **Quit** — exit (the supervised node dies with the shell).

## Presence

With an `owner_token` the shell follows the primary's presence (the snapshot
and the `/events` feed described in `docs/en/presence.md`) and shows it in the
tray:

- **Tooltip** — a short status: `Bastion — 2 sessions working`,
  `Bastion — approval waiting: Bash git push…`, `Bastion — idle`.
- **Icon** — turns amber while an approval or a question waits for you, and
  back when nothing does.
- **Notification** — a native notification, once per request, for each new
  approval: "Bastion needs your approval" with the tool and its input.
- **Approve / Deny from the tray** — every pending approval gets a submenu
  (`<tool> <input>`) with **Approve** and **Deny**, which answer it through
  `POST /presence/action`. The item disappears when the approval is resolved,
  from here or from any other surface.

The feed reconnects with backoff if the primary is unreachable. Without an
`owner_token` the shell shows no presence and logs that once.

It is a **separate package**, not part of the `bastion-agent` crate: the
headless daemon must never pull a GUI toolkit into its build. `[workspace]`
in its `Cargo.toml` keeps it out of any parent workspace.

## Configure

`shell.json` in the per-user config dir (`%APPDATA%\bastion\bastion\shell.json`
on Windows):

```json
{
  "primary_url": "https://linux-box.tailnet.ts.net:8443",
  "owner_token": "<paired-device token>",
  "node_command": ["bastion", "node", "run"],
  "stop_command": [],
  "notifications": true
}
```

- `primary_url` — the WebView loads `<primary_url>/app`.
- `owner_token` — injected into the app's `localStorage` so it starts signed
  in. Optional; without it the app shows its own connect screen.
- `node_command` — what the shell launches and supervises. Omit if a service
  runs the node; then the shell only shows the web app.
- `stop_command` — what **Stop node** runs. Empty: kill the supervised child.
- `notifications` — native notification for each new approval. Default `true`;
  set `false` to keep the tray tooltip, icon and Approve/Deny items only.

## Build

```
cargo build --release            # inside desktop/shell
```

Windows uses WebView2 (present on Windows 10/11). `tao`/`wry` also build on
Linux (webkit2gtk) and macOS (WKWebView) where those system libraries exist.

CI builds this crate on Windows in the `desktop-shell` job (clippy with
`-D warnings`, a release build, the exe uploaded as `bastion-shell-windows`).
The job is `continue-on-error`: it is visible on every run but does not turn
the pipeline red, because a native GUI's `tao`/`wry`/`tray-icon` versions can
break on their own. Build it locally on Windows before a release.

## Install (per user, no admin — BMD-28)

```
powershell -ExecutionPolicy Bypass -File install.ps1 `
    -Exe .\target\release\bastion-shell.exe `
    -PrimaryUrl https://linux-box.tailnet.ts.net:8443 `
    -OwnerToken <paired-device-token> -Autostart
```

Installs under `%LOCALAPPDATA%\Bastion`, writes `shell.json`, adds a Start-Menu
shortcut, and (with `-Autostart`) a per-user `HKCU\...\Run` entry. No
administrator rights.

### Signing

BMD-28 wants a signed executable (or MSIX). Signing is a release step outside
this script: sign `bastion-shell.exe` with `signtool` (an EV or organization
certificate) before distributing it, or wrap it in a signed MSIX. `install.ps1`
installs whatever exe it is given — sign first, then install.
