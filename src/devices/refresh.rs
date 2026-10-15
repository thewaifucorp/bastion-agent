//! Who may refresh a subscription credential on this installation (BMD-32).
//!
//! A provider that detects refresh-token reuse revokes the whole token family
//! when two devices refresh the same credential. So only the primary of the
//! current epoch refreshes: a node never does, and a primary stops the moment
//! its [`EpochFence`] learns that a newer epoch exists
//! ([`EpochFence::may_refresh`]) — an ex-primary that comes back online after
//! a promotion never spends the new primary's refresh token.

use std::sync::Arc;

use bastion_mesh::devices::fence::EpochFence;

use super::state::{DeviceState, LocalRole};

/// This installation's standing to refresh (or otherwise spend upstream) a
/// subscription credential.
#[derive(Debug, Clone, Default)]
pub enum RefreshAuthority {
    /// Multi-device is off or not set up: this is the owner's only device.
    #[default]
    Standalone,
    /// The owner's primary. Refreshes while its fence is open; the fence is
    /// shared with the hub and the event log, so a newer epoch reported by a
    /// node closes all three at once.
    Primary(Arc<EpochFence>),
    /// A node. The primary holds the credential lifecycle (BMD-30).
    Node,
}

/// Why a refresh was refused before any upstream call.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum RefreshRefused {
    #[error(
        "this device was primary of epoch {epoch} and a newer epoch exists; \
         only the current primary refreshes credentials (BMD-32)"
    )]
    SupersededPrimary { epoch: u64 },
    #[error("this device is a node; only the owner's primary refreshes credentials (BMD-32)")]
    Node,
}

impl RefreshAuthority {
    /// The authority the device state on disk gives, for a process that does
    /// not hold a live [`super::primary::PrimaryDevices`] (the `bastion auth`
    /// CLI, or a daemon on an installation that is a node). A primary whose
    /// registry already records a newer epoch starts closed, exactly as
    /// [`super::primary::PrimaryDevices::load`] does.
    pub fn from_state(state: &DeviceState) -> anyhow::Result<Self> {
        match state.role()? {
            None => Ok(Self::Standalone),
            Some(LocalRole::Node) => Ok(Self::Node),
            Some(LocalRole::Primary { epoch }) => {
                let fence = Arc::new(EpochFence::new(epoch));
                if let Some(registry) = state.registry()? {
                    fence.observe(registry.current_epoch());
                }
                Ok(Self::Primary(fence))
            }
        }
    }

    /// `Ok` when this installation may refresh now. Checked on every call,
    /// not once at startup: the fence can close while the daemon runs.
    pub fn check(&self) -> Result<(), RefreshRefused> {
        match self {
            Self::Standalone => Ok(()),
            Self::Primary(fence) if fence.may_refresh() => Ok(()),
            Self::Primary(fence) => Err(RefreshRefused::SupersededPrimary {
                epoch: fence.epoch(),
            }),
            Self::Node => Err(RefreshRefused::Node),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn standalone_always_refreshes() {
        assert_eq!(RefreshAuthority::Standalone.check(), Ok(()));
    }

    #[test]
    fn a_node_never_refreshes() {
        assert_eq!(RefreshAuthority::Node.check(), Err(RefreshRefused::Node));
    }

    #[test]
    fn a_primary_stops_refreshing_once_a_newer_epoch_exists() {
        let fence = Arc::new(EpochFence::new(2));
        let authority = RefreshAuthority::Primary(fence.clone());
        assert_eq!(authority.check(), Ok(()));

        fence.observe(3);
        assert_eq!(
            authority.check(),
            Err(RefreshRefused::SupersededPrimary { epoch: 2 })
        );

        // Promoted again: the fence reopens and so does refresh.
        fence.promote_to(4);
        assert_eq!(authority.check(), Ok(()));
    }

    #[test]
    fn state_without_a_role_is_standalone() {
        let dir = tempfile::tempdir().unwrap();
        let state = DeviceState::open(dir.path()).unwrap();
        assert!(matches!(
            RefreshAuthority::from_state(&state).unwrap(),
            RefreshAuthority::Standalone
        ));
    }

    #[test]
    fn state_as_node_is_node() {
        let dir = tempfile::tempdir().unwrap();
        let state = DeviceState::open(dir.path()).unwrap();
        state.set_role(LocalRole::Node).unwrap();
        assert_eq!(
            RefreshAuthority::from_state(&state).unwrap().check(),
            Err(RefreshRefused::Node)
        );
    }
}
