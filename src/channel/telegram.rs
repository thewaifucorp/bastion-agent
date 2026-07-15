// Telegram long-poll channel (CHAN-02).
//
// Security: chat_id is mapped to a trusted owner_id via OwnerMap (CR-03).
// Messages from chat_ids not in the map are silently dropped (no reply, no session).
// Exponential backoff on get_updates errors (CR-06).
use crate::channel::{Channel, OwnerMap};
use bastion_runtime::agent::handle::AgentHandle;

/// Telegram long-poll channel (CHAN-02).
pub struct TelegramChannel {
    pub(crate) token: String,
    pub(crate) default_persona: Option<String>,
    /// Trusted chat_id (as string) → owner_id map. Unmapped chats are silently dropped (CR-03).
    pub(crate) owner_map: OwnerMap,
}

impl TelegramChannel {
    /// Build from the `TELEGRAM_BOT_TOKEN` environment variable.  Errors if not set.
    /// Never logs the token (T-02-23 / Pitfall 7).
    pub fn from_env() -> anyhow::Result<Self> {
        let token = std::env::var("TELEGRAM_BOT_TOKEN")
            .map_err(|_| anyhow::anyhow!("TELEGRAM_BOT_TOKEN not set"))?;
        Ok(Self {
            token,
            default_persona: None,
            owner_map: OwnerMap::default(),
        })
    }

    /// Set the default persona for this channel (CHAN-04).
    pub fn with_default_persona(mut self, persona: impl Into<String>) -> Self {
        self.default_persona = Some(persona.into());
        self
    }

    /// Configure the trusted chat_id→owner_id map. Without this, all messages are dropped.
    pub fn with_owner_map(mut self, map: OwnerMap) -> Self {
        self.owner_map = map;
        self
    }
}

#[async_trait::async_trait]
impl Channel for TelegramChannel {
    async fn run(self: Box<Self>, agent: AgentHandle) -> anyhow::Result<()> {
        telegram_loop(&self.token, agent, &self.owner_map).await
    }

    fn default_persona(&self) -> Option<&str> {
        self.default_persona.as_deref()
    }
}

/// Process a single update (text + chat_id) through the AgentHandle.
/// Returns Err if the sender is not in the owner map (CR-03: reject unknown senders).
/// Factored out for unit testing without a live bot token.
pub async fn handle_update(
    text: String,
    chat_id: String,
    agent: &AgentHandle,
    owner_map: &OwnerMap,
) -> anyhow::Result<String> {
    let owner = owner_map
        .resolve(&chat_id)
        .ok_or_else(|| anyhow::anyhow!("chat_id {chat_id} not in owner map — rejecting (CR-03)"))?
        .to_owned();
    agent.ask(text, owner).await
}

async fn telegram_loop(
    token: &str,
    agent: AgentHandle,
    owner_map: &OwnerMap,
) -> anyhow::Result<()> {
    use frankenstein::client_reqwest::Bot;
    use frankenstein::methods::{GetUpdatesParams, SendMessageParams};
    use frankenstein::updates::UpdateContent;
    use frankenstein::AsyncTelegramApi;
    use tokio::time::{sleep, Duration};

    // Never log the token (T-02-23).
    let bot = Bot::new(token);
    let mut offset: i64 = 0;

    // CR-06: bounded exponential backoff on error (1s → 2s → 4s → … capped at 30s).
    // Reset to 0 on the first successful get_updates response.
    let mut backoff_secs: u64 = 0;

    loop {
        let params = GetUpdatesParams::builder()
            .offset(offset)
            .timeout(30_u32)
            .build();

        let updates = match bot.get_updates(&params).await {
            Ok(resp) => {
                // Success: reset backoff.
                backoff_secs = 0;
                resp.result
            }
            Err(e) => {
                // CR-06: exponential backoff. T-02-23: the error may embed the bot token in the
                // request URL — redact the token before logging so it never lands in the log.
                let redacted = e.to_string().replace(token, "***TOKEN***");
                tracing::warn!(
                    event = "telegram_get_updates_error",
                    error = %redacted,
                    backoff_secs,
                );
                // Apply current backoff, then double (capped at 30s).
                let wait = backoff_secs.max(1);
                sleep(Duration::from_secs(wait)).await;
                backoff_secs = (wait * 2).min(30);
                continue;
            }
        };

        for update in updates {
            // Pitfall 2: advance offset FIRST so a malformed update never loops forever.
            offset = i64::from(update.update_id) + 1;

            if let UpdateContent::Message(msg) = &update.content {
                let Some(text) = &msg.text else { continue };
                let chat_id = msg.chat.id.to_string();

                let reply =
                    match handle_update(text.clone(), chat_id.clone(), &agent, owner_map).await {
                        Ok(r) => r,
                        Err(e) => {
                            // CR-03: unknown sender — warn and skip silently (no reply to unknown chats).
                            if e.to_string().contains("not in owner map") {
                                tracing::warn!(
                                    event = "telegram_handle_update_error",
                                    chat_id = %chat_id,
                                    error = %e
                                );
                                continue;
                            }
                            // M3: log turn_error WITHOUT conversation content (Pitfall 4 — never include
                            // user_input or response_text in the log event).
                            tracing::error!(
                                event = "turn_error",
                                chat_id = %chat_id,
                            );
                            // M3: map error to friendly message — NEVER include e.to_string() (no stack
                            // trace or internal details to the user).
                            match e.downcast_ref::<bastion_types::BastionError>() {
                            Some(bastion_types::BastionError::PrivacyEgressBlocked) => {
                                "Não posso responder com este provider (restrição de privacidade)."
                                    .to_owned()
                            }
                            _ => "Tive um problema neste turn. Use /logs para detalhes.".to_owned(),
                        }
                        }
                    };

                let send_params = SendMessageParams::builder()
                    .chat_id(msg.chat.id)
                    .text(reply)
                    .build();

                if let Err(e) = bot.send_message(&send_params).await {
                    tracing::warn!("Telegram send_message error for chat {chat_id}: {e}");
                }
            }
            // Non-message updates: warn and skip (T-02-26, mirror McpClient warn+skip).
            // offset already advanced above.
        }
    }
}

// ─── tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::channel::OwnerMap;
    use bastion_runtime::agent::handle;
    use tokio::sync::mpsc;

    /// Stub consumer: replies "echo:{text}".
    fn stub_consumer(mut rx: mpsc::Receiver<bastion_runtime::agent::handle::AgentRequest>) {
        tokio::spawn(async move {
            while let Some(req) = rx.recv().await {
                let _ = req.reply.send(Ok(format!("echo:{}", req.text)));
            }
        });
    }

    #[tokio::test]
    async fn handle_update_routes_known_chat_to_agent() {
        let (h, rx) = handle::channel();
        stub_consumer(rx);
        let map = OwnerMap::from_pairs(&[("42", "mario")]);

        let reply = handle_update("ping".into(), "42".into(), &h, &map)
            .await
            .unwrap();
        assert_eq!(reply, "echo:ping");
    }

    #[tokio::test]
    async fn handle_update_rejects_unknown_chat() {
        let (h, rx) = handle::channel();
        stub_consumer(rx);
        let map = OwnerMap::from_pairs(&[("42", "mario")]);

        let result = handle_update("ping".into(), "999".into(), &h, &map).await;
        assert!(result.is_err(), "unknown chat must be rejected");
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("not in owner map"), "error message: {msg}");
    }

    #[tokio::test]
    async fn handle_update_empty_map_rejects_all() {
        let (h, rx) = handle::channel();
        stub_consumer(rx);
        let map = OwnerMap::default();

        let result = handle_update("ping".into(), "42".into(), &h, &map).await;
        assert!(result.is_err());
    }
}
