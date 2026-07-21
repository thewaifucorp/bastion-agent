//! Outbound webhook delivery (US — External Control Plane and SDK, Phase 4:
//! "Webhooks"). HMAC-SHA256-signed, at-least-once, exponential backoff.
//!
//! `SqliteWebhookDeliveryStore` is a durable queue — same conventions as
//! every other store in this module (own table on the shared session-db
//! file, `spawn_blocking` + sync `rusqlite`). [`run_delivery_loop`] mirrors
//! `adaptive::schedule::run_scheduler`'s tick-sweep-advance shape: tick,
//! sweep due deliveries, attempt each, advance (`mark_delivered` or
//! `mark_failed_and_reschedule`) — see that function's doc comment for the
//! precedent this one is structurally copied from.
//!
//! Signature header is `X-Bastion-Signature: sha256=<hex>`, the same
//! `sha256=<hex>` shape `channel::whatsapp`'s INBOUND `X-Hub-Signature-256`
//! verification already uses — this module is the OUTBOUND mirror of that
//! convention, not a new one.

use std::sync::Arc;
use std::time::Duration;

use hmac::{Hmac, Mac};
use rusqlite::Connection;
use sha2::Sha256;
use tokio::task::spawn_blocking;
use tokio::time::{interval, MissedTickBehavior};

use super::dto::TaskEventEnvelope;
use super::webhook_subscription::SqliteWebhookSubscriptionStore;

type HmacSha256 = Hmac<Sha256>;

/// Total delivery attempts allowed before a delivery is abandoned (marked
/// `dead`, no further retries) — one initial attempt plus
/// `BACKOFF_SCHEDULE_SECS.len()` retries. At that schedule's rate this spans
/// ~3.4 hours of retrying before giving up — generous for a
/// personal-agent-scale deployment without retrying forever.
const MAX_ATTEMPTS: u32 = 7;

/// Exponential backoff delays (seconds), indexed by the attempt-count
/// BEFORE this failure (0-indexed: `BACKOFF_SCHEDULE_SECS[0]` is the delay
/// after the 1st failure, before the 2nd attempt). One entry short of
/// `MAX_ATTEMPTS` by design — the delay array only needs to cover the gaps
/// BETWEEN attempts (`MAX_ATTEMPTS - 1` of them); the `MAX_ATTEMPTS`-th
/// failure marks the delivery dead instead of consulting this array again
/// (see `mark_failed_and_reschedule`, which is what
/// `exhausting_all_attempts_marks_the_delivery_dead` pins).
const BACKOFF_SCHEDULE_SECS: [i64; (MAX_ATTEMPTS - 1) as usize] = [30, 120, 600, 1800, 3600, 7200];

fn open_conn(path: &str) -> rusqlite::Result<Connection> {
    let conn = Connection::open(path)?;
    conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA busy_timeout=5000;")?;
    Ok(conn)
}

const SCHEMA_SQL: &str = "
    PRAGMA journal_mode=WAL;
    PRAGMA busy_timeout=5000;

    CREATE TABLE IF NOT EXISTS webhook_deliveries (
        id                TEXT    PRIMARY KEY,
        subscription_id   TEXT    NOT NULL,
        target_url        TEXT    NOT NULL,
        secret            TEXT    NOT NULL,
        event_json        TEXT    NOT NULL,
        attempt_count      INTEGER NOT NULL,
        next_attempt_at   INTEGER NOT NULL,
        status            TEXT    NOT NULL,
        last_error        TEXT,
        created_at        INTEGER NOT NULL
    );
    CREATE INDEX IF NOT EXISTS idx_webhook_delivery_due
        ON webhook_deliveries(status, next_attempt_at);
";

fn now_nanos() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos() as i64
}

fn uuid_like_id() -> String {
    use rand::RngCore;
    let mut bytes = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut bytes);
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// `X-Bastion-Signature: sha256=<hex>` value over the exact bytes that will
/// be sent as the request body — sign the same bytes you send, always
/// (a receiver verifies against the raw body, not a re-serialized copy that
/// could differ in whitespace/key order).
pub fn sign_payload(secret: &str, body: &[u8]) -> anyhow::Result<String> {
    let mut mac = HmacSha256::new_from_slice(secret.as_bytes())
        .map_err(|e| anyhow::anyhow!("invalid hmac key: {e}"))?;
    mac.update(body);
    let hex: String = mac
        .finalize()
        .into_bytes()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    Ok(format!("sha256={hex}"))
}

#[derive(Debug, Clone)]
struct DueDelivery {
    id: String,
    subscription_id: String,
    target_url: String,
    secret: String,
    event_json: String,
    attempt_count: u32,
}

#[derive(Clone)]
pub struct SqliteWebhookDeliveryStore {
    db_path: String,
}

impl SqliteWebhookDeliveryStore {
    pub fn new(db_path: impl Into<String>) -> Self {
        Self {
            db_path: db_path.into(),
        }
    }

    pub async fn init_schema(&self) -> anyhow::Result<()> {
        let path = self.db_path.clone();
        spawn_blocking(move || {
            let conn = open_conn(&path)?;
            conn.execute_batch(SCHEMA_SQL)?;
            Ok::<_, anyhow::Error>(())
        })
        .await?
    }

    /// Enqueue one delivery attempt, due immediately (`next_attempt_at =
    /// now`) — the sweep loop picks it up on its next tick.
    pub async fn enqueue(
        &self,
        subscription_id: &str,
        target_url: &str,
        secret: &str,
        event: &TaskEventEnvelope,
    ) -> anyhow::Result<()> {
        let path = self.db_path.clone();
        let id = uuid_like_id();
        let subscription_id = subscription_id.to_string();
        let target_url = target_url.to_string();
        let secret = secret.to_string();
        let event_json = serde_json::to_string(event)?;
        let now = now_nanos();
        spawn_blocking(move || {
            let conn = open_conn(&path)?;
            conn.execute(
                "INSERT INTO webhook_deliveries \
                    (id, subscription_id, target_url, secret, event_json, attempt_count, \
                     next_attempt_at, status, last_error, created_at) \
                 VALUES (?1, ?2, ?3, ?4, ?5, 0, ?6, 'pending', NULL, ?6)",
                rusqlite::params![id, subscription_id, target_url, secret, event_json, now],
            )?;
            Ok::<_, anyhow::Error>(())
        })
        .await?
    }

    /// Count of deliveries currently in `pending` status (due now or later —
    /// not filtered by `next_attempt_at`, unlike [`Self::due`]). A general
    /// queue-depth accessor; also what
    /// `tests/control_plane_routes.rs`'s event-emission test asserts against
    /// to prove a mutation route actually enqueued something.
    pub async fn count_pending(&self) -> anyhow::Result<i64> {
        let path = self.db_path.clone();
        spawn_blocking(move || {
            let conn = open_conn(&path)?;
            let count: i64 = conn.query_row(
                "SELECT COUNT(*) FROM webhook_deliveries WHERE status = 'pending'",
                [],
                |r| r.get(0),
            )?;
            Ok::<_, anyhow::Error>(count)
        })
        .await?
    }

    /// Every `pending` delivery due at or before `now_nanos`, oldest first.
    /// NOT owner-scoped — a global sweep, same shape as
    /// `schedule::SqliteScheduleStore::due`.
    async fn due(&self, now_nanos: i64) -> anyhow::Result<Vec<DueDelivery>> {
        let path = self.db_path.clone();
        spawn_blocking(move || {
            let conn = open_conn(&path)?;
            let mut stmt = conn.prepare(
                "SELECT id, subscription_id, target_url, secret, event_json, attempt_count \
                 FROM webhook_deliveries \
                 WHERE status = 'pending' AND next_attempt_at <= ?1 \
                 ORDER BY next_attempt_at ASC",
            )?;
            let rows = stmt
                .query_map(rusqlite::params![now_nanos], |row| {
                    Ok(DueDelivery {
                        id: row.get(0)?,
                        subscription_id: row.get(1)?,
                        target_url: row.get(2)?,
                        secret: row.get(3)?,
                        event_json: row.get(4)?,
                        attempt_count: row.get::<_, i64>(5)? as u32,
                    })
                })?
                .collect::<Result<Vec<_>, _>>()?;
            Ok::<_, anyhow::Error>(rows)
        })
        .await?
    }

    async fn mark_delivered(&self, id: &str) -> anyhow::Result<()> {
        let path = self.db_path.clone();
        let id = id.to_string();
        spawn_blocking(move || {
            let conn = open_conn(&path)?;
            conn.execute(
                "UPDATE webhook_deliveries SET status = 'delivered' WHERE id = ?1",
                rusqlite::params![id],
            )?;
            Ok::<_, anyhow::Error>(())
        })
        .await?
    }

    /// Record a failed attempt: bump `attempt_count`, either schedule the
    /// next try per [`BACKOFF_SCHEDULE_SECS`] or, past [`MAX_ATTEMPTS`],
    /// mark the delivery `dead` (no further retries — a receiver that is
    /// down for ~9 hours straight needs operator attention, not an infinite
    /// queue).
    async fn mark_failed_and_reschedule(&self, id: &str, error: &str) -> anyhow::Result<()> {
        let path = self.db_path.clone();
        let id = id.to_string();
        let error = error.to_string();
        spawn_blocking(move || {
            let conn = open_conn(&path)?;
            let attempt_count: i64 = conn.query_row(
                "SELECT attempt_count FROM webhook_deliveries WHERE id = ?1",
                rusqlite::params![id],
                |r| r.get(0),
            )?;
            let next_index = attempt_count as usize;
            if next_index >= BACKOFF_SCHEDULE_SECS.len() {
                conn.execute(
                    "UPDATE webhook_deliveries SET status = 'dead', attempt_count = attempt_count + 1, \
                     last_error = ?1 WHERE id = ?2",
                    rusqlite::params![error, id],
                )?;
            } else {
                let delay_nanos = BACKOFF_SCHEDULE_SECS[next_index] * 1_000_000_000;
                let next_attempt_at = now_nanos() + delay_nanos;
                conn.execute(
                    "UPDATE webhook_deliveries SET attempt_count = attempt_count + 1, \
                     next_attempt_at = ?1, last_error = ?2 WHERE id = ?3",
                    rusqlite::params![next_attempt_at, error, id],
                )?;
            }
            Ok::<_, anyhow::Error>(())
        })
        .await?
    }
}

/// POST one signed event to one subscriber. Deliberately does NOT re-run the
/// SSRF guard (see `webhook_subscription`'s module doc for why once, at
/// registration, is this module's chosen tradeoff) and does NOT follow
/// redirects (`redirect::Policy::none()` — same discipline
/// `adaptive::browser::HttpFetchBackend` uses, so a compromised/misconfigured
/// receiver can't 3xx-bounce the delivery to an internal target).
async fn deliver_one(
    client: &reqwest::Client,
    target_url: &str,
    secret: &str,
    event_json: &str,
) -> anyhow::Result<()> {
    let signature = sign_payload(secret, event_json.as_bytes())?;
    let resp = client
        .post(target_url)
        .header("content-type", "application/json")
        .header("x-bastion-signature", signature)
        .body(event_json.to_string())
        .send()
        .await
        .map_err(|e| anyhow::anyhow!("request failed: {e}"))?;

    if !resp.status().is_success() {
        anyhow::bail!("receiver returned {}", resp.status());
    }
    Ok(())
}

/// Background delivery loop — spawned once at daemon startup, runs until
/// dropped. Ticks every `tick`, sweeps due deliveries, attempts each via
/// [`deliver_one`], advances via [`SqliteWebhookDeliveryStore::mark_delivered`]/
/// [`SqliteWebhookDeliveryStore::mark_failed_and_reschedule`]. Mirrors
/// `adaptive::schedule::run_scheduler`'s exact tick/sweep/advance shape.
pub async fn run_delivery_loop(store: Arc<SqliteWebhookDeliveryStore>, tick: Duration) {
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(10))
        .build()
        .expect("reqwest client builds");

    let mut ticker = interval(tick);
    ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);
    loop {
        ticker.tick().await;
        let due = match store.due(now_nanos()).await {
            Ok(due) => due,
            Err(e) => {
                tracing::warn!(target: "bastion::webhook_delivery", event = "due_query_failed", error = %e);
                continue;
            }
        };
        for delivery in due {
            match deliver_one(
                &client,
                &delivery.target_url,
                &delivery.secret,
                &delivery.event_json,
            )
            .await
            {
                Ok(()) => {
                    tracing::info!(
                        event = "webhook_delivered",
                        delivery_id = %delivery.id,
                        subscription_id = %delivery.subscription_id,
                    );
                    if let Err(e) = store.mark_delivered(&delivery.id).await {
                        tracing::warn!(event = "webhook_mark_delivered_failed", error = %e);
                    }
                }
                Err(e) => {
                    tracing::warn!(
                        event = "webhook_delivery_failed",
                        delivery_id = %delivery.id,
                        subscription_id = %delivery.subscription_id,
                        attempt = delivery.attempt_count,
                        error = %e,
                    );
                    if let Err(e2) = store
                        .mark_failed_and_reschedule(&delivery.id, &e.to_string())
                        .await
                    {
                        tracing::warn!(event = "webhook_mark_failed_failed", error = %e2);
                    }
                }
            }
        }
    }
}

/// Look up active subscriptions for `owner` matching `event_type` and
/// enqueue a delivery for each — the glue between a mutation route's
/// successful write and the delivery queue. Failures here are logged, never
/// propagated: a webhook delivery hiccup must not fail the API call that
/// triggered it (spec: "A disconnected callback endpoint cannot stall task
/// execution").
pub async fn enqueue_event_for_subscribers(
    subscription_store: &SqliteWebhookSubscriptionStore,
    delivery_store: &SqliteWebhookDeliveryStore,
    owner: &str,
    event: &TaskEventEnvelope,
) {
    let matches = match subscription_store
        .active_matching(owner, &event.event_type)
        .await
    {
        Ok(m) => m,
        Err(e) => {
            tracing::warn!(event = "webhook_subscription_lookup_failed", owner, error = %e);
            return;
        }
    };
    for sub in matches {
        if let Err(e) = delivery_store
            .enqueue(&sub.id, &sub.target_url, &sub.secret, event)
            .await
        {
            tracing::warn!(
                event = "webhook_enqueue_failed",
                subscription_id = %sub.id,
                error = %e,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::NamedTempFile;

    fn sample_event() -> TaskEventEnvelope {
        TaskEventEnvelope {
            event_id: "evt_1".into(),
            event_type: "task.created".into(),
            schema_version: 1,
            task_id: "task_1".into(),
            revision: 1,
            occurred_at: 1,
            payload: serde_json::json!({}),
        }
    }

    async fn make_store() -> (NamedTempFile, SqliteWebhookDeliveryStore) {
        let f = NamedTempFile::new().expect("tempfile");
        let path = f.path().to_str().expect("utf8 path").to_owned();
        let store = SqliteWebhookDeliveryStore::new(path);
        store.init_schema().await.expect("init_schema");
        (f, store)
    }

    #[test]
    fn sign_payload_is_deterministic_and_hex_prefixed() {
        let sig1 = sign_payload("secret", b"body").unwrap();
        let sig2 = sign_payload("secret", b"body").unwrap();
        assert_eq!(sig1, sig2);
        assert!(sig1.starts_with("sha256="));
        assert_eq!(sig1.len(), "sha256=".len() + 64);
    }

    #[test]
    fn sign_payload_differs_for_different_secrets_or_bodies() {
        let base = sign_payload("secret", b"body").unwrap();
        assert_ne!(base, sign_payload("other-secret", b"body").unwrap());
        assert_ne!(base, sign_payload("secret", b"different-body").unwrap());
    }

    #[tokio::test]
    async fn enqueue_then_due_returns_it_immediately() {
        let (_f, store) = make_store().await;
        store
            .enqueue(
                "sub1",
                "https://example.com/hook",
                "secret",
                &sample_event(),
            )
            .await
            .expect("enqueue");
        let due = store.due(now_nanos()).await.expect("due");
        assert_eq!(due.len(), 1);
        assert_eq!(due[0].subscription_id, "sub1");
        assert_eq!(due[0].attempt_count, 0);
    }

    #[tokio::test]
    async fn mark_delivered_removes_it_from_due() {
        let (_f, store) = make_store().await;
        store
            .enqueue(
                "sub1",
                "https://example.com/hook",
                "secret",
                &sample_event(),
            )
            .await
            .expect("enqueue");
        let due = store.due(now_nanos()).await.expect("due");
        store.mark_delivered(&due[0].id).await.expect("mark");

        let due_after = store.due(now_nanos()).await.expect("due after");
        assert!(due_after.is_empty());
    }

    #[tokio::test]
    async fn mark_failed_reschedules_into_the_future_not_immediately_due() {
        let (_f, store) = make_store().await;
        store
            .enqueue(
                "sub1",
                "https://example.com/hook",
                "secret",
                &sample_event(),
            )
            .await
            .expect("enqueue");
        let due = store.due(now_nanos()).await.expect("due");
        store
            .mark_failed_and_reschedule(&due[0].id, "connection refused")
            .await
            .expect("mark failed");

        // Not due right now (backoff pushed it into the future).
        let due_now = store.due(now_nanos()).await.expect("due now");
        assert!(due_now.is_empty(), "must not be immediately retryable");

        // Due once we look far enough into the future.
        let due_later = store
            .due(now_nanos() + 200 * 1_000_000_000)
            .await
            .expect("due later");
        assert_eq!(due_later.len(), 1);
        assert_eq!(due_later[0].attempt_count, 1);
    }

    #[tokio::test]
    async fn exhausting_all_attempts_marks_the_delivery_dead() {
        let (_f, store) = make_store().await;
        store
            .enqueue(
                "sub1",
                "https://example.com/hook",
                "secret",
                &sample_event(),
            )
            .await
            .expect("enqueue");
        let due = store.due(now_nanos()).await.expect("due");
        let id = due[0].id.clone();

        for _ in 0..MAX_ATTEMPTS {
            store
                .mark_failed_and_reschedule(&id, "still failing")
                .await
                .expect("mark failed");
        }

        // Even looking arbitrarily far into the future, a dead delivery
        // never becomes due again.
        let due_far_future = store
            .due(now_nanos() + 1_000_000 * 1_000_000_000)
            .await
            .expect("due far future");
        assert!(
            due_far_future.is_empty(),
            "dead deliveries are never retried"
        );

        let conn = rusqlite::Connection::open(&store.db_path).unwrap();
        let status: String = conn
            .query_row(
                "SELECT status FROM webhook_deliveries WHERE id = ?1",
                rusqlite::params![id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(status, "dead");
    }

    #[tokio::test]
    async fn deliver_one_sends_a_verifiable_signature_to_a_local_server() {
        // Deliberately calls `deliver_one` directly rather than going
        // through `SqliteWebhookSubscriptionStore::issue` (SSRF-gated,
        // would reject this loopback test server) — see the module doc on
        // why the SSRF guard lives at subscription-creation time only, not
        // inside `deliver_one` itself.
        use axum::extract::State;
        use axum::routing::post;
        use std::sync::Arc as StdArc;
        use tokio::sync::Mutex;

        // clippy::type_complexity
        type Received = StdArc<Mutex<Option<(String, String)>>>;

        let received: Received = StdArc::new(Mutex::new(None));
        let received_clone = received.clone();

        async fn handler(
            State(received): State<Received>,
            headers: axum::http::HeaderMap,
            body: axum::body::Bytes,
        ) -> &'static str {
            let sig = headers
                .get("x-bastion-signature")
                .and_then(|v| v.to_str().ok())
                .unwrap_or_default()
                .to_string();
            let body_str = String::from_utf8_lossy(&body).to_string();
            *received.lock().await = Some((sig, body_str));
            "ok"
        }

        let app = axum::Router::new()
            .route("/hook", post(handler))
            .with_state(received_clone);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        let client = reqwest::Client::new();
        let event_json = serde_json::to_string(&sample_event()).unwrap();
        deliver_one(
            &client,
            &format!("http://{addr}/hook"),
            "my-secret",
            &event_json,
        )
        .await
        .expect("delivery to local test server succeeds");

        let (sig, body) = received
            .lock()
            .await
            .clone()
            .expect("server received a request");
        assert_eq!(
            body, event_json,
            "body sent must be byte-identical to what was signed"
        );
        let expected_sig = sign_payload("my-secret", event_json.as_bytes()).unwrap();
        assert_eq!(sig, expected_sig);
    }
}
