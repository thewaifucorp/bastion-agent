// Discord channel via serenity (CHAN-03).
//
// Security: msg.author.id is mapped to a trusted owner_id via OwnerMap (CR-03).
// Messages from user_ids not in the map are silently dropped (no reply, no session) —
// mirrors telegram.rs::handle_update. The bot never replies to its own (or another
// bot's) messages (T-10-05-02 — self-reply-loop DoS guard). DISCORD_BOT_TOKEN is never
// logged, even inside a serenity error's Display string (T-10-05-03).
//
// NOTE (10-RESEARCH.md Pitfall 3): GatewayIntents::MESSAGE_CONTENT below must ALSO be
// enabled in the Discord Developer Portal (Bot -> Privileged Gateway Intents) — the
// code-side intent alone is not sufficient; msg.content is empty otherwise.
use crate::channel::{Channel, OwnerMap};
use bastion_runtime::agent::handle::AgentHandle;

/// Discord bot channel (CHAN-03), backed by serenity's gateway client.
pub struct DiscordChannel {
    pub(crate) token: String,
    pub(crate) default_persona: Option<String>,
    /// Trusted Discord user_id (as string) → owner_id map. Unmapped senders are
    /// silently dropped (CR-03).
    pub(crate) owner_map: OwnerMap,
}

impl DiscordChannel {
    /// Build from the `DISCORD_BOT_TOKEN` environment variable. Errors if not set.
    /// Never logs the token (mirrors `TelegramChannel::from_env`, T-02-23).
    pub fn from_env() -> anyhow::Result<Self> {
        let token = std::env::var("DISCORD_BOT_TOKEN")
            .map_err(|_| anyhow::anyhow!("DISCORD_BOT_TOKEN not set"))?;
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

    /// Configure the trusted Discord user_id → owner_id map. Without this, all
    /// messages are dropped.
    pub fn with_owner_map(mut self, map: OwnerMap) -> Self {
        self.owner_map = map;
        self
    }
}

#[async_trait::async_trait]
impl Channel for DiscordChannel {
    async fn run(self: Box<Self>, agent: AgentHandle) -> anyhow::Result<()> {
        discord_loop(&self.token, agent, &self.owner_map).await
    }

    fn default_persona(&self) -> Option<&str> {
        self.default_persona.as_deref()
    }
}

/// Resolve a Discord user_id to an owner via the OwnerMap and forward the message to
/// the shared AgentLoop. Returns Err whose message contains "not in owner map" when
/// the sender is unknown (CR-03: reject unknown senders, mirrors telegram.rs's
/// `handle_update`). Factored out for unit testing without a live bot token.
///
/// SEC-05/D-09: `is_public_channel` — true for any guild/public-channel message,
/// false for a DM — is threaded to `ask_with_trust`. A public channel is
/// readable by anyone in the guild (untrusted, quarantines tool dispatch); a
/// DM is a 1:1 conversation with the authenticated owner (trusted, unchanged).
pub async fn handle_discord_message(
    text: String,
    discord_user_id: String,
    is_public_channel: bool,
    agent: &AgentHandle,
    owner_map: &OwnerMap,
) -> anyhow::Result<String> {
    let owner = owner_map
        .resolve(&discord_user_id)
        .ok_or_else(|| {
            anyhow::anyhow!(
                "discord_user_id {discord_user_id} not in owner map — rejecting (CR-03)"
            )
        })?
        .to_owned();
    agent.ask_with_trust(text, owner, is_public_channel).await
}

async fn discord_loop(token: &str, agent: AgentHandle, owner_map: &OwnerMap) -> anyhow::Result<()> {
    use serenity::all::{Client, GatewayIntents};

    // Pitfall 3: MESSAGE_CONTENT is a privileged intent — requesting it here is
    // necessary but not sufficient; it must ALSO be enabled in the Developer Portal.
    let intents = GatewayIntents::GUILD_MESSAGES
        | GatewayIntents::MESSAGE_CONTENT
        | GatewayIntents::DIRECT_MESSAGES;

    let mut client = Client::builder(token, intents)
        .event_handler(Handler {
            agent,
            owner_map: owner_map.clone(),
        })
        .await?;

    if let Err(e) = client.start().await {
        // T-10-05-03: the error may embed the bot token — redact before logging/bailing.
        let redacted = e.to_string().replace(token, "***TOKEN***");
        tracing::error!(event = "discord_start_error", error = %redacted);
        anyhow::bail!("discord client error: {redacted}");
    }
    Ok(())
}

/// serenity event handler bridging the Discord gateway to the shared AgentHandle.
struct Handler {
    agent: AgentHandle,
    owner_map: OwnerMap,
}

#[serenity::async_trait]
impl serenity::client::EventHandler for Handler {
    async fn message(
        &self,
        ctx: serenity::client::Context,
        msg: serenity::model::channel::Message,
    ) {
        // T-10-05-02: never process the bot's own (or another bot's) messages — Discord
        // delivers the bot's own sends back through the same event stream, and replying
        // to a bot could exhaust rate limits / budget in a self-reply loop.
        if msg.author.bot {
            return;
        }

        // SEC-05/D-09: serenity's `guild_id` is `None` for a DM and `Some(_)`
        // for any guild (public) channel — the single, explicitly-named
        // classification call site (T-11-08-03).
        let is_public_channel = msg.guild_id.is_some();

        let reply = match handle_discord_message(
            msg.content.clone(),
            msg.author.id.to_string(),
            is_public_channel,
            &self.agent,
            &self.owner_map,
        )
        .await
        {
            Ok(reply) => reply,
            Err(e) => {
                // CR-03: unknown sender — warn and skip silently (no reply).
                if e.to_string().contains("not in owner map") {
                    tracing::warn!(
                        event = "discord_handle_message_error",
                        user_id = %msg.author.id,
                        error = %e
                    );
                    return;
                }
                // M3: log turn_error WITHOUT conversation content.
                tracing::error!(event = "turn_error", user_id = %msg.author.id);
                // M3: map error to a friendly message — never leak e.to_string().
                match e.downcast_ref::<bastion_types::BastionError>() {
                    Some(bastion_types::BastionError::PrivacyEgressBlocked) => {
                        "Não posso responder com este provider (restrição de privacidade)."
                            .to_owned()
                    }
                    _ => "Tive um problema neste turn. Use /logs para detalhes.".to_owned(),
                }
            }
        };

        if let Err(e) = msg.channel_id.say(&ctx.http, reply).await {
            tracing::warn!(event = "discord_send_error", error = %e);
        }
    }

    async fn ready(&self, _ctx: serenity::client::Context, ready: serenity::model::gateway::Ready) {
        tracing::info!(event = "discord_ready", user = %ready.user.name);
    }
}

// ─── tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
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
    async fn handle_discord_message_routes_known_user_to_agent() {
        let (h, rx) = handle::channel();
        stub_consumer(rx);
        let map = OwnerMap::from_pairs(&[("111222333", "mario")]);

        let reply = handle_discord_message("ping".into(), "111222333".into(), false, &h, &map)
            .await
            .unwrap();
        assert_eq!(reply, "echo:ping");
    }

    #[tokio::test]
    async fn handle_discord_message_rejects_unknown_user() {
        let (h, rx) = handle::channel();
        stub_consumer(rx);
        let map = OwnerMap::from_pairs(&[("111222333", "mario")]);

        let result =
            handle_discord_message("ping".into(), "999999999".into(), false, &h, &map).await;
        assert!(result.is_err(), "unknown discord user must be rejected");
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("not in owner map"), "error message: {msg}");
    }

    #[tokio::test]
    async fn handle_discord_message_empty_map_rejects_all() {
        let (h, rx) = handle::channel();
        stub_consumer(rx);
        let map = OwnerMap::default();

        let result =
            handle_discord_message("ping".into(), "111222333".into(), false, &h, &map).await;
        assert!(result.is_err());
    }

    /// Plan 11-08 (SEC-05/D-09): a message from a public (non-DM) Discord
    /// context — `is_public_channel == true` — must reach the agent marked
    /// untrusted.
    #[tokio::test]
    async fn handle_discord_message_public_channel_marks_untrusted_true() {
        let (h, mut rx) = handle::channel();
        let map = OwnerMap::from_pairs(&[("111222333", "mario")]);

        let task = tokio::spawn(async move {
            handle_discord_message("ping".into(), "111222333".into(), true, &h, &map).await
        });

        let req = rx.recv().await.expect("request must arrive");
        assert!(
            req.untrusted,
            "a public (non-DM) Discord message must be untrusted"
        );
        let _ = req.reply.send(Ok("ok".into()));
        task.await.unwrap().unwrap();
    }

    /// Counterpart: a Discord DM (`is_public_channel == false`) must NOT be
    /// marked untrusted.
    #[tokio::test]
    async fn handle_discord_message_dm_marks_untrusted_false() {
        let (h, mut rx) = handle::channel();
        let map = OwnerMap::from_pairs(&[("111222333", "mario")]);

        let task = tokio::spawn(async move {
            handle_discord_message("ping".into(), "111222333".into(), false, &h, &map).await
        });

        let req = rx.recv().await.expect("request must arrive");
        assert!(!req.untrusted, "a Discord DM must be trusted");
        let _ = req.reply.send(Ok("ok".into()));
        task.await.unwrap().unwrap();
    }
}
