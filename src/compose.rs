//! Shared locator for the Bastion Docker Compose project directory.
//!
//! Both the TUI (auto-starting the local runtime) and CLI subcommands that
//! shell out to `docker compose` (e.g. `bastion connect`) need to find the
//! same project directory — one resolution order here means a command run
//! from any cwd behaves consistently instead of each caller guessing on its
//! own.

use std::path::{Path, PathBuf};

const COMPOSE_FILES: &[&str] = &[
    "compose.yaml",
    "compose.yml",
    "docker-compose.yaml",
    "docker-compose.yml",
];

/// Walk `start` and its ancestors looking for a Compose file, returning the
/// first directory that contains one.
pub fn find_compose_dir(start: &Path) -> Option<PathBuf> {
    start.ancestors().find_map(|dir| {
        COMPOSE_FILES
            .iter()
            .any(|name| dir.join(name).is_file())
            .then(|| dir.to_path_buf())
    })
}

/// Resolve the Bastion Compose project directory, in priority order:
///
/// 1. `BASTION_COMPOSE_DIR` — explicit override, for callers whose cwd and
///    install location don't line up with the other two heuristics.
/// 2. Walking up from the current directory for a Compose file — the common
///    case of running `bastion` from inside the project checkout.
/// 3. The installer's default install dir (`${XDG_DATA_HOME:-~/.local/share}/bastion`,
///    the same path `installer.sh` uses), if it actually contains a Compose
///    file — covers the CLI shim installed by `installer.sh` being invoked
///    from an unrelated directory.
pub fn locate_project_dir() -> Option<PathBuf> {
    if let Some(dir) = std::env::var("BASTION_COMPOSE_DIR")
        .ok()
        .map(PathBuf::from)
        .filter(|dir| dir.is_dir())
    {
        return Some(dir);
    }

    if let Some(dir) = std::env::current_dir()
        .ok()
        .and_then(|cwd| find_compose_dir(&cwd))
    {
        return Some(dir);
    }

    let data_home = std::env::var("XDG_DATA_HOME")
        .ok()
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var("HOME")
                .ok()
                .map(|home| PathBuf::from(home).join(".local/share"))
        })?;
    let install_dir = data_home.join("bastion");
    COMPOSE_FILES
        .iter()
        .any(|name| install_dir.join(name).is_file())
        .then_some(install_dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compose_search_walks_parent_directories() {
        let temp = tempfile::tempdir().unwrap();
        let nested = temp.path().join("a/b");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::write(temp.path().join("docker-compose.yml"), "services: {}").unwrap();
        assert_eq!(find_compose_dir(&nested), Some(temp.path().to_path_buf()));
    }
}
