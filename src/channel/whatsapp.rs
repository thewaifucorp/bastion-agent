// WhatsApp Cloud API channel (CHAN-01).
//
// Security: inbound webhook signature (X-Hub-Signature-256) MUST be verified via
// constant-time HMAC-SHA256 comparison BEFORE any JSON parsing of the request body
// (T-10-04-01 / Pitfall 1 — mirrors webhook.rs::ingest_handler's #mesh-ingest-401
// raw-bytes-first ordering). Sender phone number is resolved to a trusted owner_id
// via OwnerMap (CR-03 / T-10-04-02); unmapped senders are rejected. Secrets
// (access_token/app_secret/verify_token) are never logged (T-10-04-04).
//
// D-01: single-tenant per instance — WhatsAppSender holds exactly one
// phone_number_id/access_token pair; multi-tenant routing is a deployment concern,
// not built here. D-02: reactive-only (24h customer-service window). D-03: direct
// Meta Cloud API via `reqwest` — no BSP/middleman crate or service in between.
use crate::channel::{Channel, OwnerMap};
use bastion_runtime::agent::handle::AgentHandle;
use hmac::{Hmac, Mac};
use sha2::Sha256;

type HmacSha256 = Hmac<Sha256>;

/// WhatsApp Cloud API sender + inbound-signature verifier.
pub struct WhatsAppSender {
    pub phone_number_id: String,
    pub access_token: String,
    pub app_secret: String,
    pub verify_token: String,
    http: reqwest::Client,
}

impl WhatsAppSender {
    /// Build from `WHATSAPP_PHONE_NUMBER_ID` / `WHATSAPP_ACCESS_TOKEN` /
    /// `WHATSAPP_APP_SECRET` / `WHATSAPP_VERIFY_TOKEN` environment variables. All 4
    /// required — fail loud (mirrors `telegram.rs::TelegramChannel::from_env`).
    /// Never logs `access_token`/`app_secret`/`verify_token`.
    pub fn from_env() -> anyhow::Result<Self> {
        let phone_number_id = std::env::var("WHATSAPP_PHONE_NUMBER_ID")
            .map_err(|_| anyhow::anyhow!("WHATSAPP_PHONE_NUMBER_ID not set"))?;
        let access_token = std::env::var("WHATSAPP_ACCESS_TOKEN")
            .map_err(|_| anyhow::anyhow!("WHATSAPP_ACCESS_TOKEN not set"))?;
        let app_secret = std::env::var("WHATSAPP_APP_SECRET")
            .map_err(|_| anyhow::anyhow!("WHATSAPP_APP_SECRET not set"))?;
        let verify_token = std::env::var("WHATSAPP_VERIFY_TOKEN")
            .map_err(|_| anyhow::anyhow!("WHATSAPP_VERIFY_TOKEN not set"))?;
        Ok(Self::new(
            phone_number_id,
            access_token,
            app_secret,
            verify_token,
        ))
    }

    /// Construct directly from already-resolved values (e.g. tests, or future
    /// config-based wiring that doesn't go through env vars).
    pub fn new(
        phone_number_id: impl Into<String>,
        access_token: impl Into<String>,
        app_secret: impl Into<String>,
        verify_token: impl Into<String>,
    ) -> Self {
        Self {
            phone_number_id: phone_number_id.into(),
            access_token: access_token.into(),
            app_secret: app_secret.into(),
            verify_token: verify_token.into(),
            // A bounded timeout is a correctness requirement, not just a nicety: without
            // one, a stalled Meta Graph API connection would hang the calling axum
            // request handler indefinitely (Rule 2 — missing timeout is a DoS risk).
            http: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(10))
                .build()
                .unwrap_or_else(|_| reqwest::Client::new()),
        }
    }

    /// Send a text message via Meta's Graph API (D-03). Only valid within WhatsApp's
    /// 24h customer-service window (D-02 — reactive-only, enforced by Meta itself).
    /// Never includes `access_token` in the bail message on failure.
    pub async fn send_text(&self, to: &str, body: &str) -> anyhow::Result<()> {
        let url = format!(
            "https://graph.facebook.com/v20.0/{}/messages",
            self.phone_number_id
        );
        let resp = self
            .http
            .post(&url)
            .bearer_auth(&self.access_token)
            .json(&serde_json::json!({
                "messaging_product": "whatsapp",
                "to": to,
                "type": "text",
                "text": { "body": body }
            }))
            .send()
            .await?;
        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            anyhow::bail!("WhatsApp send failed: {status} — {text}");
        }
        Ok(())
    }

    /// Verify Meta's `X-Hub-Signature-256` header over the raw request body.
    /// Comparison is constant-time via `Hmac::verify_slice` — NEVER a manual `==`
    /// byte comparison (10-RESEARCH.md Don't-Hand-Roll; T-10-04-01).
    pub(crate) fn verify_signature(&self, body: &[u8], signature_header: &str) -> bool {
        let Some(hex_sig) = signature_header.strip_prefix("sha256=") else {
            return false;
        };
        let Some(decoded) = hex_decode(hex_sig) else {
            return false;
        };
        let Ok(mut mac) = HmacSha256::new_from_slice(self.app_secret.as_bytes()) else {
            return false;
        };
        mac.update(body);
        mac.verify_slice(&decoded).is_ok()
    }
}

/// Minimal hex decoder for the signature header. No external crate needed for this
/// narrow, non-cryptographic parsing step — the actual sensitive comparison happens
/// via constant-time `Hmac::verify_slice`, not this function.
fn hex_decode(s: &str) -> Option<Vec<u8>> {
    if !s.len().is_multiple_of(2) {
        return None;
    }
    (0..s.len())
        .step_by(2)
        .map(|i| {
            s.get(i..i + 2)
                .and_then(|byte| u8::from_str_radix(byte, 16).ok())
        })
        .collect()
}

/// Bundles what the webhook route handlers need to process WhatsApp messages —
/// passed as ONE parameter into `serve_with_mesh` rather than two.
#[derive(Clone)]
pub struct WhatsAppConfig {
    pub owner_map: OwnerMap,
    pub sender: std::sync::Arc<WhatsAppSender>,
}

/// Resolve `from_phone` via `owner_map`, forward `text` to the shared `AgentHandle`.
/// Returns `Err` (message containing `"not in owner map"`) for unmapped senders
/// (CR-03). Mirrors `telegram.rs::handle_update`'s shape exactly.
pub async fn handle_whatsapp_message(
    from_phone: String,
    text: String,
    agent: &AgentHandle,
    owner_map: &OwnerMap,
) -> anyhow::Result<String> {
    let owner = owner_map
        .resolve(&from_phone)
        .ok_or_else(|| anyhow::anyhow!("phone {from_phone} not in owner map — rejecting (CR-03)"))?
        .to_owned();
    agent.ask(text, owner).await
}

/// WhatsApp Cloud API channel (CHAN-01). `run()` is a thin no-op — inbound messages
/// arrive via the `POST /whatsapp/webhook` route mounted on the existing webhook
/// axum `Router` (10-RESEARCH.md Pattern 1), never a poll loop. This type exists
/// only so `WhatsAppChannel` satisfies the `Channel` trait for uniform
/// daemon-startup logging (wired in Plan 10-09).
pub struct WhatsAppChannel {
    pub(crate) default_persona: Option<String>,
}

#[async_trait::async_trait]
impl Channel for WhatsAppChannel {
    async fn run(self: Box<Self>, _agent: AgentHandle) -> anyhow::Result<()> {
        Ok(())
    }

    fn default_persona(&self) -> Option<&str> {
        self.default_persona.as_deref()
    }
}

// ─── tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use bastion_runtime::agent::handle;
    use tokio::sync::mpsc;

    fn test_sender() -> WhatsAppSender {
        WhatsAppSender::new(
            "test-phone-id",
            "test-access-token",
            "test-app-secret",
            "test-verify-token",
        )
    }

    fn sign(secret: &str, body: &[u8]) -> String {
        let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).expect("hmac key");
        mac.update(body);
        let digest = mac.finalize().into_bytes();
        let hex: String = digest.iter().map(|b| format!("{b:02x}")).collect();
        format!("sha256={hex}")
    }

    /// Test 1: verify_signature returns true for a body + a correctly-computed
    /// sha256=<hex hmac> signature string.
    #[test]
    fn verify_signature_accepts_valid_signature() {
        let sender = test_sender();
        let body = b"hello world";
        let sig = sign(&sender.app_secret, body);
        assert!(sender.verify_signature(body, &sig));
    }

    /// Test 2: verify_signature returns false with one hex char flipped.
    #[test]
    fn verify_signature_rejects_tampered_signature() {
        let sender = test_sender();
        let body = b"hello world";
        let mut sig = sign(&sender.app_secret, body);
        let last = sig.pop().expect("non-empty signature");
        let flipped = if last == '0' { '1' } else { '0' };
        sig.push(flipped);
        assert!(!sender.verify_signature(body, &sig));
    }

    /// Test 3: verify_signature returns false when the "sha256=" prefix is missing.
    #[test]
    fn verify_signature_rejects_missing_prefix() {
        let sender = test_sender();
        let body = b"hello world";
        let sig = sign(&sender.app_secret, body);
        let no_prefix = sig.strip_prefix("sha256=").expect("prefix present");
        assert!(!sender.verify_signature(body, no_prefix));
    }

    fn stub_consumer(mut rx: mpsc::Receiver<bastion_runtime::agent::handle::AgentRequest>) {
        tokio::spawn(async move {
            while let Some(req) = rx.recv().await {
                let _ = req.reply.send(Ok(format!("echo:{}", req.text)));
            }
        });
    }

    /// Test 4: known phone number routes to the agent and returns its reply.
    #[tokio::test]
    async fn handle_whatsapp_message_routes_known_phone_to_agent() {
        let (h, rx) = handle::channel();
        stub_consumer(rx);
        let map = OwnerMap::from_pairs(&[("+5511999999999", "mario")]);

        let reply =
            handle_whatsapp_message("+5511999999999".to_string(), "ping".to_string(), &h, &map)
                .await
                .unwrap();
        assert_eq!(reply, "echo:ping");
    }

    /// Test 5: unmapped phone number returns Err containing "not in owner map" (CR-03).
    #[tokio::test]
    async fn handle_whatsapp_message_rejects_unmapped_phone() {
        let (h, rx) = handle::channel();
        stub_consumer(rx);
        let map = OwnerMap::from_pairs(&[("+5511999999999", "mario")]);

        let result =
            handle_whatsapp_message("+0000000000".to_string(), "ping".to_string(), &h, &map).await;
        assert!(result.is_err(), "unmapped phone must be rejected");
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("not in owner map"), "error message: {msg}");
    }
}
