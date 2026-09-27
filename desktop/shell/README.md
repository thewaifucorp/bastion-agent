# bastion-shell — the Bastion desktop shell

Slice 3 of the multi-device spec (`multi-device-brain-and-nodes.md`): the
casca desktop. BMD-15, BMD-27, BMD-28.

It is a tray icon plus one window that embeds the primary's web app (`/app`)
in a WebView2 — **no chat or memory UI of its own** (BMD-27). The tray has:

- **Open Bastion** — show/focus the window;
- **Stop node** — cut the local node now: kill the supervised `bastion node
  run`, or run the configured `stop_command` (BMD-15);
- **Quit** — exit (the supervised node dies with the shell).

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
  "stop_command": []
}
```

- `primary_url` — the WebView loads `<primary_url>/app`.
- `owner_token` — injected into the app's `localStorage` so it starts signed
  in. Optional; without it the app shows its own connect screen.
- `node_command` — what the shell launches and supervises. Omit if a service
  runs the node; then the shell only shows the web app.
- `stop_command` — what **Stop node** runs. Empty: kill the supervised child.

## Build

```
cargo build --release            # inside desktop/shell
```

Windows uses WebView2 (present on Windows 10/11). `tao`/`wry` also build on
Linux (webkit2gtk) and macOS (WKWebView) where those system libraries exist.

This crate is **not** in the Agent's Windows CI job yet: it is a native GUI
whose exact `tao`/`wry`/`tray-icon` versions want a manual build before being
gated in CI, so an unverified version bump cannot turn the pipeline red. Build
it locally on Windows before a release.

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
