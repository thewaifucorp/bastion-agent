//! Helpers for reading/writing Control Plane metadata inside
//! `bastion_runtime::task::TaskCase.business_state` (US — External Control
//! Plane and SDK, Phase 3: "Mutations, idempotency, OCC and audit events").
//!
//! `business_state` is host-owned opaque JSON the kernel never interprets
//! (`OpaqueState`, `contract.rs`). `agent::task_command::steer` already
//! establishes a convention for it: a JSON **array** of tagged note objects
//! (`{"steer": "..."}`), appended to on every steer, starting from `null` ->
//! `[]` or wrapping a lone pre-existing value into `[that_value]` (see
//! `task_command.rs:141-149`). Anything this module writes must survive that
//! convention being applied later by the TUI/chat `steer` path (and vice
//! versa) — so `set_external_ref` writes a single tagged object using the
//! SAME "wrap into an array on next append" shape, and `steer_note` here is
//! a byte-for-byte mirror of `task_command::steer`'s append logic so the two
//! code paths can freely interleave on the same task without clobbering each
//! other.

use serde_json::Value;

const EXTERNAL_REF_KEY: &str = "control_plane_external_ref";
const STEER_KEY: &str = "steer";

/// Set `external_ref` on a **brand-new** case's `business_state` (never
/// called on an existing case — creation only, so there is no prior value to
/// preserve here; `steer_note` is what handles append-without-clobber on an
/// existing one).
pub fn new_business_state(external_ref: Option<&str>) -> Value {
    match external_ref {
        Some(r) => Value::Array(vec![serde_json::json!({ EXTERNAL_REF_KEY: r })]),
        None => Value::Null,
    }
}

/// Read `external_ref` back out of a case's `business_state`, tolerating
/// every shape the field can be in: untouched (`null`), API-created with a
/// ref (an array containing the tagged object), or that same array after one
/// or more TUI/chat `steer` calls appended more notes onto it. Best-effort —
/// an unrecognized shape (host state this module didn't write) returns
/// `None` rather than erroring; `business_state` carries no schema guarantee.
pub fn external_ref(business_state: &Value) -> Option<String> {
    let array = business_state.as_array()?;
    array.iter().find_map(|note| {
        note.get(EXTERNAL_REF_KEY)
            .and_then(Value::as_str)
            .map(str::to_owned)
    })
}

/// Append a steer note, exactly mirroring `task_command::steer`'s append
/// logic (`task_command.rs:141-149`) so a Control Plane `:steer` call and a
/// TUI `/task steer` call interleave safely on the same `business_state`.
pub fn append_steer_note(business_state: Value, guidance: &str) -> Value {
    let mut notes = match business_state {
        Value::Array(a) => a,
        Value::Null => Vec::new(),
        other => vec![other],
    };
    notes.push(serde_json::json!({ STEER_KEY: guidance }));
    Value::Array(notes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_business_state_none_is_null() {
        assert_eq!(new_business_state(None), Value::Null);
    }

    #[test]
    fn external_ref_round_trips_through_new_business_state() {
        let state = new_business_state(Some("paperclip-issue-42"));
        assert_eq!(external_ref(&state).as_deref(), Some("paperclip-issue-42"));
    }

    #[test]
    fn external_ref_absent_on_untouched_null_state() {
        assert_eq!(external_ref(&Value::Null), None);
    }

    #[test]
    fn external_ref_survives_a_steer_note_appended_afterward() {
        let created = new_business_state(Some("paperclip-issue-42"));
        let steered = append_steer_note(created, "focus on the auth bug first");
        assert_eq!(
            external_ref(&steered).as_deref(),
            Some("paperclip-issue-42"),
            "a TUI steer call must not clobber the external_ref an API create set"
        );
        // And the steer note itself is readable back out, same shape task_command uses.
        let array = steered.as_array().unwrap();
        assert!(
            array
                .iter()
                .any(|n| n.get("steer").and_then(Value::as_str)
                    == Some("focus on the auth bug first"))
        );
    }

    #[test]
    fn append_steer_note_wraps_a_lone_pre_existing_object_like_task_command_does() {
        let pre_existing = serde_json::json!({ "something_else": "value" });
        let result = append_steer_note(pre_existing.clone(), "guidance");
        assert_eq!(
            result,
            Value::Array(vec![pre_existing, serde_json::json!({"steer": "guidance"})])
        );
    }

    #[test]
    fn multiple_steer_notes_accumulate_in_order() {
        let mut state = new_business_state(None);
        state = append_steer_note(state, "first");
        state = append_steer_note(state, "second");
        let array = state.as_array().unwrap();
        assert_eq!(array.len(), 2);
        assert_eq!(array[0]["steer"], "first");
        assert_eq!(array[1]["steer"], "second");
    }
}
