//! Capability bridge for coding-agent session activity.
//!
//! This capability can update only local companion state. It cannot invoke
//! tools, read memory, access the network, or alter runtime permissions.

use async_trait::async_trait;
use bastion_runtime::capability::{Capability, InvokeCtx};
use serde_json::{json, Value};

pub struct CompanionEventCapability {
    schema: Value,
}

impl CompanionEventCapability {
    pub fn new() -> Self {
        Self {
            schema: json!({
                "type": "object",
                "properties": {
                    "event": {
                        "type": "string",
                        "enum": ["session-start", "activity", "session-stop"]
                    },
                    "source": {
                        "type": "string",
                        "description": "Agent source, for example claude, codex, or opencode",
                        "minLength": 1,
                        "maxLength": 32,
                        "pattern": "^[A-Za-z0-9._-]+$"
                    }
                },
                "required": ["event", "source"],
                "additionalProperties": false
            }),
        }
    }
}

impl Default for CompanionEventCapability {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Capability for CompanionEventCapability {
    fn name(&self) -> &str {
        "bastion_companion_event"
    }

    fn description(&self) -> &str {
        "Record coding-agent session activity for Bastion's local companion wellbeing loop"
    }

    fn input_schema(&self) -> &Value {
        &self.schema
    }

    async fn invoke(&self, args: Value, _ctx: &InvokeCtx) -> anyhow::Result<Value> {
        let event = args
            .get("event")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow::anyhow!("event is required"))?;
        let source = args
            .get("source")
            .and_then(Value::as_str)
            .filter(|source| !source.trim().is_empty() && source.len() <= 32)
            .ok_or_else(|| anyhow::anyhow!("source must contain 1-32 characters"))?;
        let message = crate::tui::companion_event(event, source)?;
        Ok(json!({ "recorded": true, "message": message }))
    }

    fn is_local(&self) -> bool {
        true
    }

    fn is_trusted(&self) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schema_is_closed_and_event_bounded() {
        assert_eq!(schema_value()["additionalProperties"], false);
        assert_eq!(schema_value()["required"], json!(["event", "source"]));
    }

    fn schema_value() -> Value {
        CompanionEventCapability::new().schema
    }
}
