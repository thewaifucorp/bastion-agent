//! Control Plane credential scopes (US — External Control Plane and SDK,
//! Phase 1: "auth scopes").
//!
//! Generalizes `mcp/server.rs`'s boolean `TokenPermissions::read_only` into a
//! set of named grants, matching the planning doc's "Identity and policy"
//! section: "Scope grants distinguish task read/create/control and webhook
//! management." Pure, no I/O — this module never touches the credential
//! store or a request; see [`super::credential`] for that.

use serde::{Deserialize, Serialize};

/// One grantable Control Plane capability. Every `/v1/*` operation (once
/// wired in a later phase) requires exactly one of these — never an implicit
/// "the token exists, so allow everything" grant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Scope {
    /// `GET /v1/tasks`, `GET /v1/tasks/{id}`, `GET /v1/tasks/{id}/attempts`.
    TasksRead,
    /// `POST /v1/tasks`.
    TasksCreate,
    /// `POST /v1/tasks/{id}:pause|:resume|:steer|:cancel`.
    TasksControl,
    /// `POST /v1/webhook-subscriptions` and its management (list/revoke, once added).
    WebhooksManage,
}

/// A credential's granted scopes. Order-insensitive, duplicate-tolerant —
/// membership is all that matters, never position or count.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ScopeSet(pub Vec<Scope>);

impl ScopeSet {
    pub fn new(scopes: impl IntoIterator<Item = Scope>) -> Self {
        Self(scopes.into_iter().collect())
    }

    pub fn has(&self, required: Scope) -> bool {
        self.0.contains(&required)
    }
}

/// A credential lacked a scope required for the operation it attempted.
/// Kept distinct from a generic `anyhow::Error` so a future HTTP handler can
/// map this specifically to `403` (never `401` — the credential DID
/// authenticate, it just isn't authorized for this operation) without string
/// matching an error message.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("credential missing required scope: {0:?}")]
pub struct MissingScope(pub Scope);

/// Enforce that `scopes` grants `required`, or return [`MissingScope`].
///
/// Free function (not a method on the credential type) so it stays testable
/// in isolation and mirrors the shape of `mcp/server.rs`'s
/// `authenticate_token` / `check_egress` — a single, explicit chokepoint
/// callers cannot route around.
pub fn require_scope(scopes: &ScopeSet, required: Scope) -> Result<(), MissingScope> {
    if scopes.has(required) {
        Ok(())
    } else {
        Err(MissingScope(required))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn has_true_for_granted_scope() {
        let scopes = ScopeSet::new([Scope::TasksRead, Scope::TasksControl]);
        assert!(scopes.has(Scope::TasksRead));
        assert!(scopes.has(Scope::TasksControl));
    }

    #[test]
    fn has_false_for_ungranted_scope() {
        let scopes = ScopeSet::new([Scope::TasksRead]);
        assert!(!scopes.has(Scope::TasksCreate));
        assert!(!scopes.has(Scope::TasksControl));
        assert!(!scopes.has(Scope::WebhooksManage));
    }

    #[test]
    fn empty_scope_set_grants_nothing() {
        let scopes = ScopeSet::default();
        assert!(!scopes.has(Scope::TasksRead));
    }

    #[test]
    fn require_scope_ok_when_granted() {
        let scopes = ScopeSet::new([Scope::TasksCreate]);
        assert!(require_scope(&scopes, Scope::TasksCreate).is_ok());
    }

    #[test]
    fn require_scope_err_when_missing() {
        let scopes = ScopeSet::new([Scope::TasksRead]);
        let err = require_scope(&scopes, Scope::WebhooksManage).unwrap_err();
        assert_eq!(err, MissingScope(Scope::WebhooksManage));
    }
}
