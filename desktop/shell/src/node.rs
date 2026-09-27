//! The local node the shell supervises, and the Stop action (BMD-15).
//!
//! The shell can launch `bastion node run` and keep the child handle; the
//! tray Stop item kills that child at once (which the node treats as its own
//! stop: cancel running calls, drop the connection). If the operator gave an
//! explicit `stop_command`, that runs instead — for a node managed as a
//! service.

use std::process::{Child, Command};
use std::sync::Mutex;

use crate::config::ShellConfig;

pub struct NodeSupervisor {
    child: Mutex<Option<Child>>,
    stop_command: Vec<String>,
}

impl NodeSupervisor {
    /// Start the node if `node_command` is set. A shell that only shows the
    /// web app (no local node) gets an idle supervisor.
    pub fn start(config: &ShellConfig) -> Self {
        let child = match config.node_command.split_first() {
            Some((program, args)) => match Command::new(program).args(args).spawn() {
                Ok(child) => Some(child),
                Err(e) => {
                    eprintln!("bastion-shell: cannot start the node: {e}");
                    None
                }
            },
            None => None,
        };
        Self {
            child: Mutex::new(child),
            stop_command: config.stop_command.clone(),
        }
    }

    pub fn running(&self) -> bool {
        let mut guard = self.child.lock().unwrap_or_else(|p| p.into_inner());
        match guard.as_mut() {
            Some(child) => matches!(child.try_wait(), Ok(None)),
            None => false,
        }
    }

    /// Cut the node now (BMD-15): kill the supervised child, or run the
    /// operator's explicit stop command.
    pub fn stop(&self) {
        if let Some((program, args)) = self.stop_command.split_first() {
            if let Err(e) = Command::new(program).args(args).status() {
                eprintln!("bastion-shell: stop command failed: {e}");
            }
            return;
        }
        let mut guard = self.child.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(mut child) = guard.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

impl Drop for NodeSupervisor {
    fn drop(&mut self) {
        // The node dies with the shell (like `--die-with-parent`).
        if let Some(mut child) = self.child.lock().unwrap_or_else(|p| p.into_inner()).take() {
            let _ = child.kill();
        }
    }
}
