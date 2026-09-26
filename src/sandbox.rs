//! Host side of `bastion-sandbox`: this binary is its own sandbox helper,
//! and the backend is detected once at startup and shared.
//!
//! Every program the daemon confines (subprocess extensions, the git pack,
//! agent harnesses) is started as `bastion __bastion-sandbox ...`;
//! [`forward_helper_invocation`] must therefore run first in `main`, before
//! the async runtime or anything else.

use std::sync::OnceLock;

use bastion_sandbox::{Sandbox, HELPER_MARKER};

use crate::config::SandboxMode;

static SANDBOX: OnceLock<Option<Sandbox>> = OnceLock::new();

/// If this process was started as the sandbox helper, become it (never
/// returns). Otherwise does nothing.
pub fn forward_helper_invocation() {
    if std::env::args_os().nth(1).as_deref() == Some(std::ffi::OsStr::new(HELPER_MARKER)) {
        bastion_sandbox::helper_main(std::env::args_os().skip(1));
    }
}

/// Detect the backend per `mode`, once. `Required` turns "no backend" into
/// a startup error; `Auto` logs it and leaves tools unconfined where they
/// can run that way (subprocess extensions still refuse); `Off` skips
/// detection entirely.
pub fn init(mode: SandboxMode) -> anyhow::Result<Option<&'static Sandbox>> {
    let detected = match mode {
        SandboxMode::Off => {
            tracing::warn!(
                event = "sandbox_disabled",
                "[sandbox] mode = \"off\": tools and agent harnesses run with the daemon's full \
                 filesystem view"
            );
            None
        }
        SandboxMode::Auto | SandboxMode::Required => {
            let helper = std::env::current_exe().map_err(|e| {
                anyhow::anyhow!("cannot locate this executable for the sandbox helper: {e}")
            })?;
            match Sandbox::detect(helper) {
                Ok(sandbox) => {
                    tracing::info!(event = "sandbox_ready", backend = ?sandbox.backend());
                    Some(sandbox)
                }
                Err(e) if mode == SandboxMode::Required => {
                    anyhow::bail!("[sandbox] mode = \"required\" but {e}")
                }
                Err(e) => {
                    tracing::warn!(
                        event = "sandbox_unavailable",
                        error = %e,
                        "tools and agent harnesses run unconfined; subprocess extensions are refused"
                    );
                    None
                }
            }
        }
    };
    Ok(SANDBOX.get_or_init(|| detected).as_ref())
}

/// [`init`] with an explicit helper instead of this executable — for tests,
/// whose binary is not `bastion` (`env!("CARGO_BIN_EXE_bastion")` is).
pub fn init_with_helper(helper: std::path::PathBuf) -> Option<&'static Sandbox> {
    SANDBOX
        .get_or_init(|| Sandbox::detect(helper).ok())
        .as_ref()
}

/// The detected sandbox, or `None` when unavailable, disabled, or not yet
/// initialized (tests that never call [`init`]).
pub fn current() -> Option<&'static Sandbox> {
    SANDBOX.get().and_then(Option::as_ref)
}

/// Resolve `name` on `PATH` the way a shell would, for programs a spec needs
/// as a path.
pub fn resolve_on_path(name: &str) -> Option<std::path::PathBuf> {
    if name.contains('/') {
        return Some(std::path::PathBuf::from(name));
    }
    std::env::var_os("PATH")
        .into_iter()
        .flat_map(|path| std::env::split_paths(&path).collect::<Vec<_>>())
        .map(|dir| dir.join(name))
        .find(|candidate| candidate.is_file())
}
