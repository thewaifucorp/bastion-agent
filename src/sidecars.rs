//! Native install: the daemon runs the Python MCP sidecars (memupalace,
//! skill-writer, self-improving, voice) itself, each confined by
//! `crate::sandbox` with **no network at all**.
//!
//! In a container deployment Compose runs them on an `internal: true`
//! network: they can talk to each other and to the daemon but not to the
//! Internet. Natively the same guarantee comes from Unix sockets: every
//! sidecar listens on `<run dir>/<name>.sock` (`MCP_UNIX_SOCKET`; see
//! [`run_dir`]), reaches
//! memupalace and the daemon's `/api/infer` through their sockets too
//! (`MEMUPALACE_SOCKET`, `CORE_GATEWAY_SOCKET`), and the daemon's MCP client
//! connects with `url = "unix:<socket>"`. No TCP port is opened, so nothing
//! outside the run directory (0700) reaches a sidecar, and a blocked network
//! cannot break them.
//!
//! Layout under `[sidecars] root` (default `<data>/sidecars`), written by the
//! native installer:
//!
//! - `src/skills/<name>/` — the code, read-only to every sidecar (kept apart
//!   from the skills directory skill-writer writes to, so no sidecar can edit
//!   another's code);
//! - `venv/<name>/` — its virtualenv;
//! - `models/<name>/` — models downloaded at install time (they cannot be
//!   fetched at runtime: there is no network);
//! - `data/<name>/` — its state; `logs/<name>.log` — its stderr.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::time::Duration;

use bastion_types::McpServerEntry;

use crate::config::SidecarsConfig;

/// The sidecars this module knows how to launch.
pub const KNOWN: &[&str] = &["memupalace", "skill-writer", "self-improving", "voice"];

/// The `[mcp.servers.<key>]` name a sidecar has in bastion.toml and in the
/// container deployment (`skill_writer`, not `skill-writer`), so native and
/// container installs register the same server names.
pub fn mcp_server_key(name: &str) -> String {
    name.replace('-', "_")
}

/// Where the daemon's own `/api/infer` listens when sidecars run natively.
pub fn infer_socket_path() -> PathBuf {
    run_dir().join("infer.sock")
}

/// Where the sockets live: `BASTION_RUN_DIR`, else `$XDG_RUNTIME_DIR/bastion`
/// (Linux: `/run/user/<uid>`, a per-user tmpfs), else
/// `<temp dir>/bastion-<uid>`. Deliberately not under the data dir: a Unix
/// socket path is limited to ~104 bytes (macOS) / 108 (Linux), and a data
/// dir under a long home path would exceed it. Created 0700 before use.
pub fn run_dir() -> PathBuf {
    resolve_run_dir(|key| std::env::var_os(key), current_uid())
}

fn resolve_run_dir(env: impl Fn(&str) -> Option<std::ffi::OsString>, uid: Option<u32>) -> PathBuf {
    let set = |key: &str| env(key).filter(|v| !v.is_empty()).map(PathBuf::from);
    if let Some(explicit) = set("BASTION_RUN_DIR") {
        return explicit;
    }
    if let Some(runtime) = set("XDG_RUNTIME_DIR") {
        return runtime.join("bastion");
    }
    let suffix = uid
        .map(|u| format!("bastion-{u}"))
        .unwrap_or_else(|| "bastion".into());
    std::env::temp_dir().join(suffix)
}

/// The operator's uid, read off their home directory (no `unsafe` libc call).
fn current_uid() -> Option<u32> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        std::env::var_os("HOME")
            .and_then(|home| std::fs::metadata(home).ok())
            .map(|m| m.uid())
    }
    #[cfg(not(unix))]
    {
        None
    }
}

/// Longest socket path both Linux (108) and macOS (104) accept, with room
/// for the terminating NUL.
const MAX_SOCKET_PATH: usize = 103;

fn socket_path_fits(path: &Path) -> bool {
    path.as_os_str().len() <= MAX_SOCKET_PATH
}

/// Paths of one installed sidecar tree.
#[derive(Debug, Clone)]
struct Layout {
    root: PathBuf,
    run: PathBuf,
    skills_dir: PathBuf,
}

impl Layout {
    fn src(&self) -> PathBuf {
        self.root.join("src")
    }
    fn code(&self, name: &str) -> PathBuf {
        self.src().join("skills").join(name)
    }
    fn python(&self, name: &str) -> PathBuf {
        self.root.join("venv").join(name).join("bin/python")
    }
    fn models(&self, name: &str) -> PathBuf {
        self.root.join("models").join(name)
    }
    fn data(&self, name: &str) -> PathBuf {
        self.root.join("data").join(name)
    }
    fn log(&self, name: &str) -> PathBuf {
        self.root.join("logs").join(format!("{name}.log"))
    }
    fn socket(&self, name: &str) -> PathBuf {
        self.run.join(format!("{name}.sock"))
    }
}

/// Everything needed to start one sidecar, before confinement.
#[derive(Debug)]
struct Launch {
    python: PathBuf,
    args: Vec<String>,
    cwd: PathBuf,
    env: BTreeMap<String, String>,
    read_only: Vec<PathBuf>,
    read_write: Vec<PathBuf>,
    /// How long a cold start may take before the socket appears (model load).
    ready_within: Duration,
}

fn path_str(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

fn launch_for(name: &str, layout: &Layout, infer_token: Option<&str>) -> Option<Launch> {
    let data = layout.data(name);
    let mut env: BTreeMap<String, String> = [
        ("PATH", "/usr/bin:/bin".to_string()),
        ("LANG", "C.UTF-8".to_string()),
        ("HOME", path_str(&data)),
        ("PYTHONPATH", path_str(&layout.src())),
        ("PYTHONDONTWRITEBYTECODE", "1".to_string()),
        ("MCP_UNIX_SOCKET", path_str(&layout.socket(name))),
        ("HF_HUB_OFFLINE", "1".to_string()),
        ("TRANSFORMERS_OFFLINE", "1".to_string()),
        ("CORE_GATEWAY_URL", "http://localhost/api/infer".to_string()),
        (
            "CORE_GATEWAY_SOCKET",
            path_str(&layout.run.join("infer.sock")),
        ),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), v))
    .collect();
    if let Some(token) = infer_token {
        env.insert("BASTION_INFER_TOKEN".to_string(), token.to_string());
    }
    let mut read_only = vec![layout.src(), layout.root.join("venv").join(name)];
    let mut read_write = vec![data.clone(), layout.run.clone()];
    let memupalace_peer = |env: &mut BTreeMap<String, String>| {
        env.insert("MEMUPALACE_URL".into(), "http://localhost/mcp".into());
        env.insert(
            "MEMUPALACE_SOCKET".into(),
            path_str(&layout.socket("memupalace")),
        );
    };
    let (args, cwd, ready_within) = match name {
        "memupalace" => {
            let models = layout.models(name);
            env.insert(
                "MEMUPALACE_CHROMA_PATH".into(),
                path_str(&data.join("chroma")),
            );
            env.insert(
                "MEMUPALACE_ONNX_MODEL_PATH".into(),
                path_str(&models.join("model_optimized.onnx")),
            );
            env.insert("MEMUPALACE_TOKENIZER_NAME".into(), path_str(&models));
            read_only.push(models);
            (
                vec!["-m".into(), "skills.memupalace.mcp_server".into()],
                layout.src(),
                Duration::from_secs(90),
            )
        }
        "voice" => {
            let models = layout.models(name);
            env.insert(
                "VOICE_WHISPER_MODEL_DIR".into(),
                path_str(&models.join("whisper")),
            );
            env.insert("VOICE_WHISPER_MODEL_SIZE".into(), "small".into());
            env.insert(
                "VOICE_KOKORO_MODEL_PATH".into(),
                path_str(&models.join("kokoro/kokoro-v1.0.onnx")),
            );
            env.insert(
                "VOICE_KOKORO_VOICES_PATH".into(),
                path_str(&models.join("kokoro/voices-v1.0.bin")),
            );
            read_only.push(models);
            (
                vec!["-m".into(), "skills.voice.mcp_server".into()],
                layout.src(),
                Duration::from_secs(120),
            )
        }
        "skill-writer" => {
            env.insert("SKILLS_DIR".into(), path_str(&layout.skills_dir));
            env.insert(
                "SKILL_WRITER_PENDING_FILE".into(),
                path_str(&data.join("pending_distillations.jsonl")),
            );
            memupalace_peer(&mut env);
            // Writes SKILL.md files, as in the container (`./skills` rw).
            read_write.push(layout.skills_dir.clone());
            (
                vec!["mcp_server.py".into()],
                layout.code(name),
                Duration::from_secs(30),
            )
        }
        "self-improving" => {
            env.insert("SKILLS_DIR".into(), path_str(&layout.skills_dir));
            env.insert(
                "SELF_SUGGESTIONS_FILE".into(),
                path_str(&data.join("suggestions.jsonl")),
            );
            memupalace_peer(&mut env);
            // Reads skills, never writes them (D-10).
            read_only.push(layout.skills_dir.clone());
            (
                vec!["mcp_server.py".into()],
                layout.code(name),
                Duration::from_secs(30),
            )
        }
        _ => return None,
    };
    // The venv's python is a symlink into the interpreter install; its
    // standard library lives next to that install's `bin/`.
    let python = layout.python(name);
    if let Some(prefix) = std::fs::canonicalize(&python)
        .ok()
        .and_then(|real| real.parent().map(Path::to_path_buf))
        .filter(|bin| bin.file_name().is_some_and(|n| n == "bin"))
        .and_then(|bin| bin.parent().map(Path::to_path_buf))
    {
        read_only.push(prefix);
    }
    Some(Launch {
        python,
        args,
        cwd,
        env,
        read_only,
        read_write,
        ready_within,
    })
}

/// The MCP entry the daemon connects through for `name`.
fn mcp_entry(name: &str, socket: &Path) -> McpServerEntry {
    serde_json::from_value(serde_json::json!({
        "url": format!("{}{}", bastion_mcp::client::UNIX_SOCKET_SCHEME, socket.display()),
        "label": name,
        // A sidecar is local by construction: it has no network.
        "is_local": true,
    }))
    .expect("McpServerEntry deserializes from url/label/is_local")
}

fn prepare_dir(path: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

/// Start every enabled sidecar under the sandbox, wait (bounded) for their
/// sockets, and return the MCP servers to connect to. Never runs a sidecar
/// unconfined: without a sandbox it logs and starts none. Each one is
/// restarted with backoff for the life of the process.
pub async fn start(cfg: &SidecarsConfig) -> HashMap<String, McpServerEntry> {
    let mut servers = HashMap::new();
    if cfg.enabled.is_empty() {
        return servers;
    }
    let Some(sandbox) = crate::sandbox::current() else {
        tracing::error!(
            event = "sidecars_not_started",
            "[sidecars] enabled but this host has no OS sandbox; sidecars never run unconfined"
        );
        return servers;
    };
    let layout = Layout {
        root: cfg
            .root
            .clone()
            .unwrap_or_else(|| crate::config::data_root().join("sidecars")),
        run: run_dir(),
        skills_dir: PathBuf::from(crate::agent::skills::skills_dir()),
    };
    if let Err(e) = prepare_dir(&layout.run) {
        tracing::error!(event = "sidecars_run_dir_failed", path = %layout.run.display(), error = %e);
        return servers;
    }
    let infer_token = {
        use bastion_types::SecretResolver;
        crate::secret::default_secret_resolver()
            .resolve("BASTION_INFER_TOKEN")
            .ok()
            .map(|v| v.expose_secret().to_string())
    };
    let mut waits = Vec::new();
    for name in &cfg.enabled {
        let Some(launch) = launch_for(name, &layout, infer_token.as_deref()) else {
            tracing::warn!(event = "sidecar_unknown", sidecar = %name, known = ?KNOWN);
            continue;
        };
        if !launch.python.exists() {
            tracing::warn!(
                event = "sidecar_not_installed",
                sidecar = %name,
                python = %launch.python.display(),
                "run the native installer to create its virtualenv"
            );
            continue;
        }
        for dir in launch.read_write.iter().chain([&layout.root.join("logs")]) {
            if let Err(e) = prepare_dir(dir) {
                tracing::warn!(event = "sidecar_dir_failed", sidecar = %name, path = %dir.display(), error = %e);
            }
        }
        let socket = layout.socket(name);
        if !socket_path_fits(&socket) {
            tracing::error!(
                event = "sidecar_socket_path_too_long",
                sidecar = %name,
                socket = %socket.display(),
                "Unix socket paths are limited to {MAX_SOCKET_PATH} bytes; set BASTION_RUN_DIR \
                 to a shorter directory"
            );
            continue;
        }
        let _ = std::fs::remove_file(&socket);
        let ready_within = launch.ready_within;
        tokio::spawn(supervise(
            name.clone(),
            launch,
            layout.log(name),
            sandbox.clone(),
        ));
        let key = mcp_server_key(name);
        servers.insert(key.clone(), mcp_entry(&key, &socket));
        waits.push((name.clone(), socket, ready_within));
    }
    for (name, socket, ready_within) in waits {
        if wait_for(&socket, ready_within).await {
            tracing::info!(event = "sidecar_ready", sidecar = %name, socket = %socket.display());
        } else {
            tracing::warn!(
                event = "sidecar_slow_start",
                sidecar = %name,
                "socket not up yet; its MCP tools appear after the next reconnect"
            );
        }
    }
    servers
}

async fn wait_for(socket: &Path, within: Duration) -> bool {
    let deadline = tokio::time::Instant::now() + within;
    while tokio::time::Instant::now() < deadline {
        if socket.exists() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    socket.exists()
}

/// Run `launch` confined, forever: restart on exit with exponential backoff
/// (1 s doubling to 60 s, reset after a minute of healthy running). Its
/// stderr goes to `log`.
async fn supervise(name: String, launch: Launch, log: PathBuf, sandbox: bastion_sandbox::Sandbox) {
    let mut backoff = Duration::from_secs(1);
    loop {
        let started = tokio::time::Instant::now();
        match spawn(&launch, &log, &sandbox) {
            Ok(mut child) => {
                tracing::info!(event = "sidecar_started", sidecar = %name, pid = ?child.id());
                let status = child.wait().await;
                tracing::warn!(event = "sidecar_exited", sidecar = %name, status = ?status);
            }
            Err(e) => {
                tracing::error!(event = "sidecar_spawn_failed", sidecar = %name, error = %e);
            }
        }
        if started.elapsed() > Duration::from_secs(60) {
            backoff = Duration::from_secs(1);
        }
        tokio::time::sleep(backoff).await;
        backoff = (backoff * 2).min(Duration::from_secs(60));
    }
}

fn spawn(
    launch: &Launch,
    log: &Path,
    sandbox: &bastion_sandbox::Sandbox,
) -> anyhow::Result<tokio::process::Child> {
    let mut spec = bastion_sandbox::SandboxSpec::new(&launch.python)
        .args(&launch.args)
        .envs(launch.env.clone())
        .cwd(&launch.cwd)
        .network(bastion_sandbox::Network::Blocked);
    for path in launch.read_only.iter().filter(|p| p.exists()) {
        spec = spec.read_only(path);
    }
    for path in &launch.read_write {
        spec = spec.read_write(path);
    }
    let stderr = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(log)?;
    let mut command = tokio::process::Command::from(sandbox.command(&spec)?);
    command
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(stderr)
        .kill_on_drop(true);
    Ok(command.spawn()?)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn layout() -> Layout {
        Layout {
            root: PathBuf::from("/data/sidecars"),
            run: PathBuf::from("/data/run"),
            skills_dir: PathBuf::from("/data/skills"),
        }
    }

    #[test]
    fn every_sidecar_talks_over_sockets_and_gets_only_its_own_paths() {
        for name in KNOWN {
            let launch = launch_for(name, &layout(), Some("tok")).expect(name);
            assert_eq!(
                launch.env["MCP_UNIX_SOCKET"],
                format!("/data/run/{name}.sock")
            );
            assert_eq!(launch.env["CORE_GATEWAY_SOCKET"], "/data/run/infer.sock");
            assert_eq!(launch.env["HF_HUB_OFFLINE"], "1");
            assert_eq!(launch.env["BASTION_INFER_TOKEN"], "tok");
            assert!(launch
                .read_write
                .contains(&PathBuf::from(format!("/data/sidecars/data/{name}"))));
            // Code is never writable, by anyone.
            assert!(!launch
                .read_write
                .iter()
                .any(|p| p.starts_with("/data/sidecars/src")));
            assert!(!launch
                .read_write
                .iter()
                .any(|p| p.starts_with("/data/sidecars/venv")));
            // Another sidecar's data is not granted.
            for other in KNOWN.iter().filter(|o| *o != name) {
                let theirs = PathBuf::from(format!("/data/sidecars/data/{other}"));
                assert!(
                    !launch.read_write.contains(&theirs) && !launch.read_only.contains(&theirs)
                );
            }
        }
    }

    #[test]
    fn only_skill_writer_may_write_skills() {
        let skills = PathBuf::from("/data/skills");
        let writer = launch_for("skill-writer", &layout(), None).unwrap();
        assert!(writer.read_write.contains(&skills));
        let reader = launch_for("self-improving", &layout(), None).unwrap();
        assert!(reader.read_only.contains(&skills) && !reader.read_write.contains(&skills));
        for name in ["memupalace", "voice"] {
            let l = launch_for(name, &layout(), None).unwrap();
            assert!(!l.read_write.contains(&skills) && !l.read_only.contains(&skills));
        }
    }

    #[test]
    fn peers_reach_memupalace_through_its_socket() {
        for name in ["skill-writer", "self-improving"] {
            let l = launch_for(name, &layout(), None).unwrap();
            assert_eq!(l.env["MEMUPALACE_SOCKET"], "/data/run/memupalace.sock");
            assert!(!l.env.contains_key("BASTION_INFER_TOKEN"));
        }
        assert!(launch_for("not-a-sidecar", &layout(), None).is_none());
    }

    #[test]
    fn run_dir_prefers_explicit_then_runtime_dir_then_a_per_user_temp_dir() {
        let env = |pairs: &'static [(&'static str, &'static str)]| {
            move |key: &str| {
                pairs
                    .iter()
                    .find(|(k, _)| *k == key)
                    .map(|(_, v)| std::ffi::OsString::from(v))
            }
        };
        assert_eq!(
            resolve_run_dir(
                env(&[
                    ("BASTION_RUN_DIR", "/r"),
                    ("XDG_RUNTIME_DIR", "/run/user/1000")
                ]),
                Some(1000)
            ),
            PathBuf::from("/r")
        );
        assert_eq!(
            resolve_run_dir(env(&[("XDG_RUNTIME_DIR", "/run/user/1000")]), Some(1000)),
            PathBuf::from("/run/user/1000/bastion")
        );
        assert_eq!(
            resolve_run_dir(env(&[]), Some(501)),
            std::env::temp_dir().join("bastion-501")
        );
    }

    #[test]
    fn socket_paths_over_the_os_limit_are_refused() {
        assert!(socket_path_fits(Path::new(
            "/run/user/1000/bastion/self-improving.sock"
        )));
        let long = format!("/{}/self-improving.sock", "x".repeat(100));
        assert!(!socket_path_fits(Path::new(&long)));
    }

    #[test]
    fn mcp_entries_use_the_unix_scheme_and_are_local() {
        assert_eq!(mcp_server_key("skill-writer"), "skill_writer");
        let e = mcp_entry("voice", Path::new("/data/run/voice.sock"));
        assert_eq!(e.url, "unix:/data/run/voice.sock");
        assert!(e.is_local);
    }
}
