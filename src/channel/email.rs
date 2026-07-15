// SMTP/IMAP email channel via lettre + async-imap (CHAN-03).
//
// Security: the `From:` header of an inbound email is UNTRUSTED input (SMTP does not
// authenticate it — anyone can claim to be anyone). It is resolved to a trusted owner_id
// via OwnerMap (CR-03) exactly like every other channel's credential. Senders whose
// address is absent from the map are silently dropped (no reply, no session) — mirrors
// telegram.rs::handle_update. EMAIL_PASSWORD is never logged, even inside an IMAP/SMTP
// error's Display string (T-10-06-02).
//
// Pitfall 5 (10-RESEARCH.md): an IMAP IDLE session left open indefinitely is silently
// dropped by many servers around the ~29-minute mark — the receive loop re-issues IDLE
// every 25 minutes, well under that threshold, and falls back to a 60s poll loop if the
// mailbox does not advertise IDLE support at all.
use crate::channel::{Channel, OwnerMap};
use bastion_runtime::agent::handle::AgentHandle;

/// Email channel (CHAN-03): SMTP send via `lettre`, IMAP receive via `async-imap`
/// (native `IDLE` with a polling fallback).
pub struct EmailChannel {
    pub(crate) imap_host: String,
    pub(crate) imap_port: u16,
    pub(crate) smtp_host: String,
    pub(crate) smtp_port: u16,
    pub(crate) username: String,
    pub(crate) password: String,
    pub(crate) default_persona: Option<String>,
    /// Trusted sender-address → owner_id map. Unmapped senders are silently
    /// dropped (CR-03).
    pub(crate) owner_map: OwnerMap,
}

impl EmailChannel {
    /// Build from `EMAIL_ADDRESS`/`EMAIL_PASSWORD`/`EMAIL_IMAP_HOST`/`EMAIL_SMTP_HOST`
    /// (required, fail loud) and `EMAIL_IMAP_PORT`/`EMAIL_SMTP_PORT` (optional,
    /// default 993/587). Never logs the password (T-10-06-02).
    pub fn from_env() -> anyhow::Result<Self> {
        let username =
            std::env::var("EMAIL_ADDRESS").map_err(|_| anyhow::anyhow!("EMAIL_ADDRESS not set"))?;
        let password = std::env::var("EMAIL_PASSWORD")
            .map_err(|_| anyhow::anyhow!("EMAIL_PASSWORD not set"))?;
        let imap_host = std::env::var("EMAIL_IMAP_HOST")
            .map_err(|_| anyhow::anyhow!("EMAIL_IMAP_HOST not set"))?;
        let imap_port = std::env::var("EMAIL_IMAP_PORT")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(993);
        let smtp_host = std::env::var("EMAIL_SMTP_HOST")
            .map_err(|_| anyhow::anyhow!("EMAIL_SMTP_HOST not set"))?;
        let smtp_port = std::env::var("EMAIL_SMTP_PORT")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(587);

        Ok(Self {
            imap_host,
            imap_port,
            smtp_host,
            smtp_port,
            username,
            password,
            default_persona: None,
            owner_map: OwnerMap::default(),
        })
    }

    /// Set the default persona for this channel (CHAN-04).
    pub fn with_default_persona(mut self, persona: impl Into<String>) -> Self {
        self.default_persona = Some(persona.into());
        self
    }

    /// Configure the trusted sender-address → owner_id map. Without this, all
    /// messages are dropped.
    pub fn with_owner_map(mut self, map: OwnerMap) -> Self {
        self.owner_map = map;
        self
    }
}

#[async_trait::async_trait]
impl Channel for EmailChannel {
    async fn run(self: Box<Self>, agent: AgentHandle) -> anyhow::Result<()> {
        email_loop(
            &self.imap_host,
            self.imap_port,
            &self.smtp_host,
            self.smtp_port,
            &self.username,
            &self.password,
            agent,
            &self.owner_map,
        )
        .await
    }

    fn default_persona(&self) -> Option<&str> {
        self.default_persona.as_deref()
    }
}

/// Parse raw RFC822 message bytes into `(from_address, body)`.
///
/// Extracts the bare email address out of either the `"Name <addr@x.com>"` or bare
/// `"addr@x.com"` form of the `From:` header, and the plain-text body (mailparse
/// decodes quoted-printable/base64 transfer encodings automatically). Bails if the
/// `From:` header is absent.
pub(crate) fn parse_email_message(raw: &[u8]) -> anyhow::Result<(String, String)> {
    use mailparse::MailHeaderMap;

    let mail = mailparse::parse_mail(raw)?;

    let from_header = mail
        .headers
        .get_first_value("From")
        .ok_or_else(|| anyhow::anyhow!("email has no From: header"))?;

    let from_address = extract_email_address(&from_header);
    let body = mail.get_body()?;

    Ok((from_address, body))
}

/// Extract the bare email address out of a `From:` header value, handling both the
/// `"Name <addr@x.com>"` display-name form and a bare `"addr@x.com"` form.
fn extract_email_address(header_value: &str) -> String {
    if let Some(start) = header_value.find('<') {
        if let Some(end) = header_value.find('>') {
            if end > start {
                return header_value[start + 1..end].trim().to_owned();
            }
        }
    }
    header_value.trim().to_owned()
}

/// Resolve a sender address to an owner via the OwnerMap and forward the message to
/// the shared AgentLoop. Returns Err whose message contains "not in owner map" when
/// the sender is unknown (CR-03: reject unknown senders — the `From:` header is
/// untrusted/spoofable input, mirrors telegram.rs's `handle_update`). Factored out
/// for unit testing without a live mailbox.
///
/// SEC-05/D-09: received email content is ALWAYS untrusted — no signature/DKIM
/// verification currently gates this, and even a known/mapped sender's BODY
/// content is untrusted (the OwnerMap only vouches for WHO is sending, never
/// for WHAT the message body says). `ask_with_trust(..., true)` quarantines the
/// agent's tool-facing dispatch for this turn.
pub async fn handle_email_message(
    from_address: String,
    text: String,
    agent: &AgentHandle,
    owner_map: &OwnerMap,
) -> anyhow::Result<String> {
    let owner = owner_map
        .resolve(&from_address)
        .ok_or_else(|| {
            anyhow::anyhow!("email address {from_address} not in owner map — rejecting (CR-03)")
        })?
        .to_owned();
    agent.ask_with_trust(text, owner, true).await
}

/// IMAP IDLE-with-poll-fallback receive loop + SMTP reply send.
///
/// Connects IMAPS, gates on `IDLE` capability (25-minute internal timeout re-issue vs
/// 60s poll fallback, Pitfall 5), fetches UNSEEN messages, routes each through
/// [`handle_email_message`], and replies via a single reused `lettre` SMTP transport.
/// Wraps the connect+login step in the same bounded exponential backoff
/// (`telegram_loop`'s shape: 1s → 2s → 4s → … capped at 30s, reset on first success)
/// so a transient IMAP/SMTP outage does not busy-loop or crash the channel.
// EmailChannel has more config fields (imap/smtp host+port, username, password) than any
// other channel in this phase — the 8-arg signature mirrors those fields 1:1 (per the
// plan's own call shape) rather than introducing a config struct for a single call site.
#[allow(clippy::too_many_arguments)]
async fn email_loop(
    imap_host: &str,
    imap_port: u16,
    smtp_host: &str,
    smtp_port: u16,
    username: &str,
    password: &str,
    agent: AgentHandle,
    owner_map: &OwnerMap,
) -> anyhow::Result<()> {
    use futures_util::TryStreamExt;
    use lettre::transport::smtp::authentication::Credentials;
    use lettre::{AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor};
    use tokio::time::{sleep, Duration};

    // Built once, reused for every reply. `starttls_relay` matches the documented
    // default port 587 (STARTTLS) — see EMAIL_SMTP_PORT's "Defaults to 587 (STARTTLS)"
    // contract; `relay()` would pair the wrong Tls mode (implicit TLS / port 465) with
    // this port, per lettre's own builder docs warning about mismatched port+Tls combos.
    let mailer: AsyncSmtpTransport<Tokio1Executor> =
        AsyncSmtpTransport::<Tokio1Executor>::starttls_relay(smtp_host)?
            .port(smtp_port)
            .credentials(Credentials::new(username.to_owned(), password.to_owned()))
            .build();

    // CR-06-style bounded exponential backoff on connect/login errors (1s -> 2s -> 4s ->
    // ... capped at 30s). Reset to 0 on the first successful login.
    let mut backoff_secs: u64 = 0;

    loop {
        let mut session = match connect_and_login(imap_host, imap_port, username, password).await {
            Ok(session) => {
                backoff_secs = 0;
                session
            }
            Err(e) => {
                // T-10-06-02: password is NEVER interpolated into this message — connect
                // errors only ever carry host/transport-level detail.
                tracing::warn!(
                    event = "email_connect_error",
                    error = %e,
                    backoff_secs,
                );
                let wait = backoff_secs.max(1);
                sleep(Duration::from_secs(wait)).await;
                backoff_secs = (wait * 2).min(30);
                continue;
            }
        };

        if let Err(e) = session.select("INBOX").await {
            tracing::warn!(event = "email_select_inbox_error", error = %e);
            continue;
        }

        let supports_idle = match session.capabilities().await {
            Ok(caps) => caps.has_str("IDLE"),
            Err(e) => {
                tracing::warn!(event = "email_capabilities_error", error = %e);
                false
            }
        };

        // Reuse the same authenticated session across many IDLE/poll cycles until a
        // hard error forces a reconnect (breaks out to the outer loop).
        'session: loop {
            if supports_idle {
                let mut idle_handle = session.idle();
                if let Err(e) = idle_handle.init().await {
                    tracing::warn!(event = "email_idle_init_error", error = %e);
                    break 'session;
                }
                // Pitfall 5: re-issue IDLE every 25 minutes, safely under the ~29-minute
                // server-side drop. Both the Timeout and NewData outcomes fall through to
                // the same UNSEEN search below.
                let (idle_wait, _stop) =
                    idle_handle.wait_with_timeout(Duration::from_secs(25 * 60));
                if let Err(e) = idle_wait.await {
                    tracing::warn!(event = "email_idle_wait_error", error = %e);
                }
                session = match idle_handle.done().await {
                    Ok(s) => s,
                    Err(e) => {
                        tracing::warn!(event = "email_idle_done_error", error = %e);
                        break 'session;
                    }
                };
            } else {
                sleep(Duration::from_secs(60)).await;
            }

            let uids = match session.search("UNSEEN").await {
                Ok(uids) => uids,
                Err(e) => {
                    tracing::warn!(event = "email_search_unseen_error", error = %e);
                    break 'session;
                }
            };

            for seq in uids {
                let messages: Vec<async_imap::types::Fetch> = match session
                    .fetch(seq.to_string(), "RFC822")
                    .await
                {
                    Ok(stream) => match stream.try_collect().await {
                        Ok(v) => v,
                        Err(e) => {
                            tracing::warn!(event = "email_fetch_collect_error", seq, error = %e);
                            continue;
                        }
                    },
                    Err(e) => {
                        tracing::warn!(event = "email_fetch_error", seq, error = %e);
                        continue;
                    }
                };

                for fetched in &messages {
                    let Some(raw) = fetched.body() else {
                        continue;
                    };

                    let (from_address, text) = match parse_email_message(raw) {
                        Ok(parsed) => parsed,
                        Err(e) => {
                            tracing::warn!(event = "email_parse_error", seq, error = %e);
                            continue;
                        }
                    };

                    let reply = match handle_email_message(
                        from_address.clone(),
                        text,
                        &agent,
                        owner_map,
                    )
                    .await
                    {
                        Ok(reply) => reply,
                        Err(e) => {
                            // CR-03: unknown sender — warn and skip silently (no reply).
                            if e.to_string().contains("not in owner map") {
                                tracing::warn!(
                                    event = "email_handle_message_error",
                                    from = %from_address,
                                    error = %e
                                );
                                continue;
                            }
                            // M3: log turn_error WITHOUT conversation content.
                            tracing::error!(event = "turn_error", from = %from_address);
                            match e.downcast_ref::<bastion_types::BastionError>() {
                                Some(bastion_types::BastionError::PrivacyEgressBlocked) => {
                                    "Não posso responder com este provider (restrição de privacidade)."
                                        .to_owned()
                                }
                                _ => "Tive um problema neste turn. Use /logs para detalhes.".to_owned(),
                            }
                        }
                    };

                    let email = match Message::builder()
                        .from(username.parse()?)
                        .to(from_address.parse()?)
                        .subject("Re: Bastion")
                        .body(reply)
                    {
                        Ok(email) => email,
                        Err(e) => {
                            tracing::warn!(event = "email_build_error", error = %e);
                            continue;
                        }
                    };

                    if let Err(e) = mailer.send(email).await {
                        tracing::warn!(event = "email_send_error", error = %e);
                    }
                }

                // Mark processed so it is not reprocessed on the next UNSEEN search.
                match session.store(seq.to_string(), "+FLAGS (\\Seen)").await {
                    Ok(stream) => {
                        if let Err(e) = stream.try_collect::<Vec<_>>().await {
                            tracing::warn!(event = "email_store_seen_error", seq, error = %e);
                        }
                    }
                    Err(e) => {
                        tracing::warn!(event = "email_store_seen_error", seq, error = %e);
                    }
                }
            }
        }
    }
}

/// Connect over IMAPS and log in, returning an authenticated `Session`.
async fn connect_and_login(
    imap_host: &str,
    imap_port: u16,
    username: &str,
    password: &str,
) -> anyhow::Result<async_imap::Session<async_native_tls::TlsStream<tokio::net::TcpStream>>> {
    let tcp_stream = tokio::net::TcpStream::connect((imap_host, imap_port)).await?;
    let tls_stream = async_native_tls::TlsConnector::new()
        .connect(imap_host, tcp_stream)
        .await?;
    let client = async_imap::Client::new(tls_stream);
    let session = client
        .login(username, password)
        .await
        .map_err(|(e, _)| anyhow::anyhow!("imap login failed: {e}"))?;
    Ok(session)
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

    const RAW_WITH_DISPLAY_NAME: &[u8] =
        b"From: Mario <mario@example.com>\r\nTo: bastion@example.com\r\nSubject: hi\r\n\r\nhello there\r\n";

    const RAW_BARE_ADDRESS: &[u8] =
        b"From: mario@example.com\r\nTo: bastion@example.com\r\nSubject: hi\r\n\r\nhello there\r\n";

    #[test]
    fn parse_email_message_extracts_address_from_display_name_form() {
        let (from, body) = parse_email_message(RAW_WITH_DISPLAY_NAME).unwrap();
        assert_eq!(from, "mario@example.com");
        assert!(body.contains("hello there"));
    }

    #[test]
    fn parse_email_message_extracts_bare_address() {
        let (from, body) = parse_email_message(RAW_BARE_ADDRESS).unwrap();
        assert_eq!(from, "mario@example.com");
        assert!(body.contains("hello there"));
    }

    #[tokio::test]
    async fn handle_email_message_routes_known_sender_to_agent() {
        let (h, rx) = handle::channel();
        stub_consumer(rx);
        let map = OwnerMap::from_pairs(&[("mario@example.com", "mario")]);

        let reply = handle_email_message("mario@example.com".into(), "ping".into(), &h, &map)
            .await
            .unwrap();
        assert_eq!(reply, "echo:ping");
    }

    #[tokio::test]
    async fn handle_email_message_rejects_unmapped_sender() {
        let (h, rx) = handle::channel();
        stub_consumer(rx);
        let map = OwnerMap::from_pairs(&[("mario@example.com", "mario")]);

        let result =
            handle_email_message("stranger@example.com".into(), "ping".into(), &h, &map).await;
        assert!(result.is_err(), "unmapped sender must be rejected");
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("not in owner map"), "error message: {msg}");
    }

    #[tokio::test]
    async fn handle_email_message_empty_map_rejects_all() {
        let (h, rx) = handle::channel();
        stub_consumer(rx);
        let map = OwnerMap::default();

        let result =
            handle_email_message("mario@example.com".into(), "ping".into(), &h, &map).await;
        assert!(result.is_err());
    }

    /// Plan 11-08 (SEC-05/D-09): received email content ALWAYS reaches the
    /// agent marked untrusted — no signature/DKIM check gates this, and even
    /// a known/mapped sender's BODY content is untrusted.
    #[tokio::test]
    async fn handle_email_message_always_marks_untrusted_true() {
        let (h, mut rx) = handle::channel();
        let map = OwnerMap::from_pairs(&[("mario@example.com", "mario")]);

        let task = tokio::spawn(async move {
            handle_email_message("mario@example.com".into(), "ping".into(), &h, &map).await
        });

        let req = rx.recv().await.expect("request must arrive");
        assert!(
            req.untrusted,
            "received email content must ALWAYS be marked untrusted (D-09), no carve-out"
        );
        let _ = req.reply.send(Ok("ok".into()));
        task.await.unwrap().unwrap();
    }
}
