// Slack Socket Mode channel via slack-morphism (CHAN-03).
//
// Security: the Slack user_id extracted from an inbound push event is mapped to a
// trusted owner_id via OwnerMap (CR-03). Messages from user_ids not in the map are
// silently dropped (no reply, no session) — mirrors telegram.rs::handle_update. The
// bot never processes its own (or another bot's) messages (sender.bot_id guard,
// mirrors discord.rs's msg.author.bot guard). Neither SLACK_BOT_TOKEN nor
// SLACK_APP_TOKEN is ever logged, even inside a slack-morphism error's Display string.
//
// NOTE (10-RESEARCH.md Pitfall 4): Socket Mode needs BOTH the bot token (xoxb-...,
// used for chat.postMessage) and the app-level token (xapp-..., used to open the
// websocket) — mixing them up fails the handshake with an unhelpful auth error.
//
// NOTE (10-RESEARCH.md Assumption A3 / plan note): slack-morphism's
// `UserCallbackFunction` is a plain `fn` pointer (not a `Fn` closure trait), so
// per-connection state (agent handle, owner map, bot token) cannot be captured by a
// closure — it is threaded through `SlackClientEventsListenerEnvironment`'s
// `with_user_state`/`SlackClientEventsUserState` storage instead, exactly the
// mechanism the crate itself provides for this.
use crate::channel::{Channel, OwnerMap};
use bastion_runtime::agent::handle::AgentHandle;
use rvstruct::ValueStruct;
use slack_morphism::prelude::*;
use std::sync::Arc;

/// Slack Socket Mode channel (CHAN-03) — no public HTTPS endpoint required.
pub struct SlackChannel {
    pub(crate) bot_token: String,
    pub(crate) app_token: String,
    pub(crate) default_persona: Option<String>,
    /// Trusted Slack user_id → owner_id map. Unmapped senders are silently dropped
    /// (CR-03).
    pub(crate) owner_map: OwnerMap,
}

impl SlackChannel {
    /// Build from the `SLACK_BOT_TOKEN` (xoxb-...) and `SLACK_APP_TOKEN` (xapp-...)
    /// environment variables. Each missing var fails loud independently, naming which
    /// one is missing (mirrors `webhook.rs`'s `APP_JWT_SECRET` fail-closed style).
    /// Neither token is ever logged.
    pub fn from_env() -> anyhow::Result<Self> {
        let bot_token = std::env::var("SLACK_BOT_TOKEN")
            .map_err(|_| anyhow::anyhow!("SLACK_BOT_TOKEN not set"))?;
        let app_token = std::env::var("SLACK_APP_TOKEN")
            .map_err(|_| anyhow::anyhow!("SLACK_APP_TOKEN not set"))?;
        Ok(Self {
            bot_token,
            app_token,
            default_persona: None,
            owner_map: OwnerMap::default(),
        })
    }

    /// Set the default persona for this channel (CHAN-04).
    pub fn with_default_persona(mut self, persona: impl Into<String>) -> Self {
        self.default_persona = Some(persona.into());
        self
    }

    /// Configure the trusted Slack user_id → owner_id map. Without this, all messages
    /// are dropped.
    pub fn with_owner_map(mut self, map: OwnerMap) -> Self {
        self.owner_map = map;
        self
    }
}

#[async_trait::async_trait]
impl Channel for SlackChannel {
    async fn run(self: Box<Self>, agent: AgentHandle) -> anyhow::Result<()> {
        slack_loop(&self.bot_token, &self.app_token, agent, &self.owner_map).await
    }

    fn default_persona(&self) -> Option<&str> {
        self.default_persona.as_deref()
    }
}

/// Resolve a Slack user_id to an owner via the OwnerMap and forward the message to the
/// shared AgentLoop. Returns Err whose message contains "not in owner map" when the
/// sender is unknown (CR-03). Factored out for unit testing without live tokens.
///
/// SEC-05/D-09: `is_public_channel` — true for any non-DM channel/group
/// message, false for a direct message — is threaded to `ask_with_trust`,
/// mirroring `handle_discord_message`'s classification.
pub async fn handle_slack_message(
    text: String,
    slack_user_id: String,
    is_public_channel: bool,
    agent: &AgentHandle,
    owner_map: &OwnerMap,
) -> anyhow::Result<String> {
    let owner = owner_map
        .resolve(&slack_user_id)
        .ok_or_else(|| {
            anyhow::anyhow!("slack_user_id {slack_user_id} not in owner map — rejecting (CR-03)")
        })?
        .to_owned();
    agent.ask_with_trust(text, owner, is_public_channel).await
}

/// Per-connection state threaded through slack-morphism's `SlackClientEventsUserState`
/// storage — required because `UserCallbackFunction` is a plain `fn` pointer and
/// cannot capture `agent`/`owner_map`/`bot_api_token` via closure (see module doc).
struct SlackHandlerState {
    agent: AgentHandle,
    owner_map: OwnerMap,
    bot_api_token: SlackApiToken,
}

async fn slack_loop(
    bot_token: &str,
    app_token: &str,
    agent: AgentHandle,
    owner_map: &OwnerMap,
) -> anyhow::Result<()> {
    let hyper_connector = SlackClientHyperConnector::new()
        .map_err(|e| anyhow::anyhow!("failed to build Slack hyper connector: {e}"))?;
    let client: Arc<SlackHyperClient> = Arc::new(SlackClient::new(hyper_connector));

    let bot_api_token = SlackApiToken::new(SlackApiTokenValue(bot_token.to_owned()));

    let listener_environment = Arc::new(
        SlackClientEventsListenerEnvironment::new(client.clone()).with_user_state(
            SlackHandlerState {
                agent,
                owner_map: owner_map.clone(),
                bot_api_token: bot_api_token.clone(),
            },
        ),
    );

    let callbacks = SlackSocketModeListenerCallbacks::new().with_push_events(on_slack_push_event);

    let socket_mode_listener = SlackClientSocketModeListener::new(
        &SlackClientSocketModeConfig::new(),
        listener_environment,
        callbacks,
    );

    let app_api_token = SlackApiToken::new(SlackApiTokenValue(app_token.to_owned()));

    socket_mode_listener
        .listen_for(&app_api_token)
        .await
        .map_err(|e| {
            // Pitfall 4 / T-10-05-03: never leak either token in the connect error.
            let redacted = e
                .to_string()
                .replace(app_token, "***TOKEN***")
                .replace(bot_token, "***TOKEN***");
            anyhow::anyhow!("slack socket mode connect error: {redacted}")
        })?;

    // Blocks until the process receives a termination signal — same "run forever"
    // contract as Discord's `client.start()` / Telegram's long-poll loop.
    socket_mode_listener.serve().await;
    Ok(())
}

/// Push-events callback registered with slack-morphism's Socket Mode listener.
/// Must be a plain (non-capturing) fn — see module doc — state comes from
/// `SlackClientEventsUserState` instead of a closure environment.
async fn on_slack_push_event(
    event: SlackPushEventCallback,
    client: Arc<SlackHyperClient>,
    states: SlackClientEventsUserState,
) -> UserCallbackResult<()> {
    let SlackEventCallbackBody::Message(msg) = event.event else {
        return Ok(());
    };
    // Never process the bot's own (or another bot's) messages — mirrors discord.rs's
    // `msg.author.bot` self-reply-loop guard.
    if msg.sender.bot_id.is_some() {
        return Ok(());
    }
    let Some(user) = msg.sender.user.clone() else {
        return Ok(());
    };
    let Some(text) = msg.content.as_ref().and_then(|c| c.text.clone()) else {
        return Ok(());
    };
    let Some(channel) = msg.origin.channel.clone() else {
        return Ok(());
    };

    let (agent, owner_map, bot_api_token) = {
        let guard = states.read().await;
        let Some(state) = guard.get_user_state::<SlackHandlerState>() else {
            tracing::error!(event = "slack_state_missing");
            return Ok(());
        };
        (
            state.agent.clone(),
            state.owner_map.clone(),
            state.bot_api_token.clone(),
        )
    };

    // SEC-05/D-09: Slack's `channel_type` is `"im"` for a direct message and
    // any other value (e.g. "channel"/"group") for a public/group context
    // (T-11-08-03). Missing `channel_type` fails toward untrusted (fail-cautious,
    // matching the codebase's existing `unwrap_or(true)` posture for ambiguous
    // trust signals — see SEC-01's `needs_approval` sourcing).
    let is_public_channel = msg
        .origin
        .channel_type
        .as_ref()
        .map(|ct| ct.value() != "im")
        .unwrap_or(true);

    let reply = match handle_slack_message(
        text,
        user.value().clone(),
        is_public_channel,
        &agent,
        &owner_map,
    )
    .await
    {
        Ok(reply) => reply,
        Err(e) => {
            // CR-03: unknown sender — warn and skip silently (no reply).
            if e.to_string().contains("not in owner map") {
                tracing::warn!(
                    event = "slack_handle_message_error",
                    user_id = %user.value(),
                    error = %e
                );
                return Ok(());
            }
            // M3: log turn_error WITHOUT conversation content.
            tracing::error!(event = "turn_error", user_id = %user.value());
            // M3: map error to a friendly message — never leak e.to_string().
            match e.downcast_ref::<bastion_types::BastionError>() {
                Some(bastion_types::BastionError::PrivacyEgressBlocked) => {
                    "Não posso responder com este provider (restrição de privacidade).".to_owned()
                }
                _ => "Tive um problema neste turn. Use /logs para detalhes.".to_owned(),
            }
        }
    };

    let session = client.open_session(&bot_api_token);
    let req =
        SlackApiChatPostMessageRequest::new(channel, SlackMessageContent::new().with_text(reply));
    if let Err(e) = session.chat_post_message(&req).await {
        tracing::warn!(event = "slack_send_error", error = %e);
    }
    Ok(())
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
    async fn handle_slack_message_routes_known_user_to_agent() {
        let (h, rx) = handle::channel();
        stub_consumer(rx);
        let map = OwnerMap::from_pairs(&[("U01ABCDEF", "mario")]);

        let reply = handle_slack_message("ping".into(), "U01ABCDEF".into(), false, &h, &map)
            .await
            .unwrap();
        assert_eq!(reply, "echo:ping");
    }

    #[tokio::test]
    async fn handle_slack_message_rejects_unknown_user() {
        let (h, rx) = handle::channel();
        stub_consumer(rx);
        let map = OwnerMap::from_pairs(&[("U01ABCDEF", "mario")]);

        let result = handle_slack_message("ping".into(), "U99ZZZZZZ".into(), false, &h, &map).await;
        assert!(result.is_err(), "unknown slack user must be rejected");
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("not in owner map"), "error message: {msg}");
    }

    #[tokio::test]
    async fn handle_slack_message_empty_map_rejects_all() {
        let (h, rx) = handle::channel();
        stub_consumer(rx);
        let map = OwnerMap::default();

        let result = handle_slack_message("ping".into(), "U01ABCDEF".into(), false, &h, &map).await;
        assert!(result.is_err());
    }

    /// Plan 11-08 (SEC-05/D-09): a message from a public (non-DM) Slack
    /// context — `is_public_channel == true` — must reach the agent marked
    /// untrusted.
    #[tokio::test]
    async fn handle_slack_message_public_channel_marks_untrusted_true() {
        let (h, mut rx) = handle::channel();
        let map = OwnerMap::from_pairs(&[("U01ABCDEF", "mario")]);

        let task = tokio::spawn(async move {
            handle_slack_message("ping".into(), "U01ABCDEF".into(), true, &h, &map).await
        });

        let req = rx.recv().await.expect("request must arrive");
        assert!(
            req.untrusted,
            "a public (non-DM) Slack message must be untrusted"
        );
        let _ = req.reply.send(Ok("ok".into()));
        task.await.unwrap().unwrap();
    }

    /// Counterpart: a Slack DM (`is_public_channel == false`) must NOT be
    /// marked untrusted.
    #[tokio::test]
    async fn handle_slack_message_dm_marks_untrusted_false() {
        let (h, mut rx) = handle::channel();
        let map = OwnerMap::from_pairs(&[("U01ABCDEF", "mario")]);

        let task = tokio::spawn(async move {
            handle_slack_message("ping".into(), "U01ABCDEF".into(), false, &h, &map).await
        });

        let req = rx.recv().await.expect("request must arrive");
        assert!(!req.untrusted, "a Slack DM must be trusted");
        let _ = req.reply.send(Ok("ok".into()));
        task.await.unwrap().unwrap();
    }
}
