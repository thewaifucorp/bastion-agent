//! US-205 — generalized personal scheduling, self-contained in the agent.
//!
//! Lets a user schedule *any* already-authorized intent to fire once
//! (`OneShot`) or on a fixed recurrence (`Every`), and have it survive a
//! restart. This is deliberately **not** Core's `CronService`: the durable
//! store and firing loop live here, owner-scoped, and the daemon supplies a
//! callback ([`run_scheduler`]'s `on_fire`) that decides what a fire actually
//! *does* (e.g. `enqueue_pursue` + `coding_cycle`, or pushing a
//! `PendingItem`). Keeping the store/loop decoupled from the task store,
//! runtime registry and the proactive-turn queue is intentional — this module
//! owns *when* a schedule fires; the daemon owns *what* firing means.
//!
//! ## Conventions (copied from `bastion-runtime`'s `SqliteTaskStore`)
//! `tokio::task::spawn_blocking` around a synchronous rusqlite `Connection`
//! (never held across an `.await`), `PRAGMA journal_mode=WAL; busy_timeout`,
//! `CREATE TABLE IF NOT EXISTS` schema init, the owner-scoped IDOR guard
//! (`WHERE id=?1 AND owner_id=?2`, bailing — never silently no-opping — on
//! zero rows changed), optimistic-concurrency (OCC) revision bumps, and
//! `serde_json` TEXT columns for the structured enums.
//!
//! ## Timezone (stubbed)
//! `ScheduleSpec::tz` is persisted but not yet honored: fire times are
//! computed purely in nanoseconds-since-epoch. DST/tz-aware slot expansion is
//! a documented TODO — it would require `chrono-tz`, which we deliberately do
//! **not** pull in here.

use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use tokio::task::spawn_blocking;
use tokio::time::{interval, MissedTickBehavior};

/// Wall-clock now as nanoseconds-since-epoch (US-205).
///
/// Mirrors `enqueue.rs`'s helper. `SystemTime` is permitted in normal runtime
/// code (the `Date::now`/`SystemTime` ban applies only to workflow scripts).
pub fn now_nanos() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos() as i64
}

/// How a schedule recurs (US-205).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ScheduleKind {
    /// Fire exactly once at `at_nanos`, then revoke.
    OneShot { at_nanos: i64 },
    /// Fire every `interval_secs` seconds, indefinitely (until revoked).
    Every { interval_secs: u64 },
}

/// What to do about fires that were missed while the scheduler was down
/// (US-205). Applied when a recurring schedule is overdue by more than one
/// interval — see [`plan_fire`] for the exact, documented semantics.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum MissedPolicy {
    /// Drop the whole missed backlog: realign to the next future slot without
    /// firing for any overdue occurrence.
    Skip,
    /// Collapse the backlog into exactly one fire, then realign.
    RunOnce,
    /// Fire up to `max` of the missed occurrences, then realign.
    CatchUpBounded { max: u32 },
}

/// A durable, owner-scoped schedule for an authorized `intent` (US-205).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScheduleSpec {
    /// Stable schedule id (also the primary key).
    pub id: String,
    /// Owner — every store method is scoped to this to prevent IDOR.
    pub owner: String,
    /// The authorized intent text to dispatch when this fires.
    pub intent: String,
    /// One-shot or recurring.
    pub kind: ScheduleKind,
    /// Missed-run policy (see [`MissedPolicy`]).
    pub missed: MissedPolicy,
    /// Optional IANA timezone name. Persisted but not yet honored — fire
    /// times are computed in nanos-since-epoch (tz-aware expansion is a TODO).
    pub tz: Option<String>,
    /// The next wall-clock instant (nanos-since-epoch) this schedule is due.
    pub next_fire_nanos: i64,
    /// Once revoked, the schedule never fires again and is skipped by [`SqliteScheduleStore::due`].
    pub revoked: bool,
    /// OCC revision, bumped on every mutation.
    pub revision: u64,
}

/// The single-interval advance rule (US-205).
///
/// `OneShot` never has a next fire (the caller revokes after it fires);
/// `Every { interval_secs }` advances one interval past `fired_at_nanos`. A
/// zero (or overflowing) interval degenerates to `None` so a mis-specified
/// schedule cannot spin the firing loop.
pub fn compute_next_fire(kind: &ScheduleKind, fired_at_nanos: i64) -> Option<i64> {
    match kind {
        ScheduleKind::OneShot { .. } => None,
        ScheduleKind::Every { interval_secs } => {
            let interval_nanos = (*interval_secs as i64).saturating_mul(1_000_000_000);
            if interval_nanos <= 0 {
                None
            } else {
                Some(fired_at_nanos.saturating_add(interval_nanos))
            }
        }
    }
}

/// The outcome of servicing one due schedule at a given `now` (US-205).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FirePlan {
    /// How many times the loop should invoke `on_fire` for this schedule now.
    pub fire_count: u32,
    /// The schedule's new `next_fire_nanos`, or `None` when it should be
    /// revoked (one-shot, or a degenerate recurrence).
    pub next_fire_nanos: Option<i64>,
}

/// Decide how to service a due schedule (US-205). Pure — no store, no daemon.
///
/// Precondition: the schedule is due (`next_fire_nanos <= now`); the firing
/// loop only calls this on rows returned by [`SqliteScheduleStore::due`].
///
/// - **`OneShot`** → fire once, revoke (`next_fire_nanos = None`).
/// - **`Every { interval }`** → let `missed_slots` be the number of scheduled
///   slots at or before `now` (`>= 1`; `>= 2` means "overdue by more than one
///   interval"). The new next fire is always the first slot strictly after
///   `now`. The count of fires depends on [`MissedPolicy`]:
///   - **`Skip`** — if overdue (`missed_slots >= 2`), fire `0` times (drop the
///     backlog); otherwise fire once.
///   - **`RunOnce`** — always fire exactly once, however large the backlog.
///   - **`CatchUpBounded { max }`** — fire `min(missed_slots, max)` times.
pub fn plan_fire(
    kind: &ScheduleKind,
    missed: &MissedPolicy,
    next_fire_nanos: i64,
    now: i64,
) -> FirePlan {
    match kind {
        ScheduleKind::OneShot { .. } => FirePlan {
            fire_count: 1,
            next_fire_nanos: None,
        },
        ScheduleKind::Every { interval_secs } => {
            let interval_nanos = (*interval_secs as i64).saturating_mul(1_000_000_000);
            if interval_nanos <= 0 {
                // Degenerate interval: fire once and revoke rather than spin.
                return FirePlan {
                    fire_count: 1,
                    next_fire_nanos: None,
                };
            }
            let delta = now.saturating_sub(next_fire_nanos).max(0);
            let missed_slots = (delta / interval_nanos) + 1;
            let next_future =
                next_fire_nanos.saturating_add(missed_slots.saturating_mul(interval_nanos));
            let fire_count = match missed {
                MissedPolicy::Skip => {
                    if missed_slots >= 2 {
                        0
                    } else {
                        1
                    }
                }
                MissedPolicy::RunOnce => 1,
                MissedPolicy::CatchUpBounded { max } => missed_slots.min(*max as i64).max(0) as u32,
            };
            FirePlan {
                fire_count,
                next_fire_nanos: Some(next_future),
            }
        }
    }
}

fn open_conn(path: &str) -> rusqlite::Result<Connection> {
    let conn = Connection::open(path)?;
    conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA busy_timeout=5000;")?;
    Ok(conn)
}

const SCHEMA_SQL: &str = "
    PRAGMA journal_mode=WAL;
    PRAGMA busy_timeout=5000;

    CREATE TABLE IF NOT EXISTS schedules (
        id                TEXT    PRIMARY KEY,
        owner_id          TEXT    NOT NULL,
        intent            TEXT    NOT NULL,
        kind_json         TEXT    NOT NULL,
        missed_json       TEXT    NOT NULL,
        tz                TEXT,
        next_fire_nanos   INTEGER NOT NULL,
        revoked           INTEGER NOT NULL,
        revision          INTEGER NOT NULL
    );
    CREATE INDEX IF NOT EXISTS idx_schedules_due
        ON schedules(revoked, next_fire_nanos);
    CREATE INDEX IF NOT EXISTS idx_schedules_owner ON schedules(owner_id);
";

const READ_COLUMNS: &str =
    "id, owner_id, intent, kind_json, missed_json, tz, next_fire_nanos, revoked, revision";

/// The raw column values of a `schedules` row, before the `serde_json` enum
/// columns are decoded (that decode can fail, so it happens outside the
/// rusqlite row closure — which may only yield `rusqlite::Result`).
struct RawScheduleRow {
    id: String,
    owner_id: String,
    intent: String,
    kind_json: String,
    missed_json: String,
    tz: Option<String>,
    next_fire_nanos: i64,
    revoked: i64,
    revision: i64,
}

fn read_spec_row(row: &rusqlite::Row) -> rusqlite::Result<RawScheduleRow> {
    Ok(RawScheduleRow {
        id: row.get(0)?,
        owner_id: row.get(1)?,
        intent: row.get(2)?,
        kind_json: row.get(3)?,
        missed_json: row.get(4)?,
        tz: row.get(5)?,
        next_fire_nanos: row.get(6)?,
        revoked: row.get(7)?,
        revision: row.get(8)?,
    })
}

fn raw_to_spec(raw: RawScheduleRow) -> anyhow::Result<ScheduleSpec> {
    Ok(ScheduleSpec {
        id: raw.id,
        owner: raw.owner_id,
        intent: raw.intent,
        kind: serde_json::from_str(&raw.kind_json)?,
        missed: serde_json::from_str(&raw.missed_json)?,
        tz: raw.tz,
        next_fire_nanos: raw.next_fire_nanos,
        revoked: raw.revoked != 0,
        revision: raw.revision as u64,
    })
}

/// SQLite-backed durable schedule store (US-205). Owns its own `schedules`
/// table — never the task tables.
#[derive(Clone)]
pub struct SqliteScheduleStore {
    db_path: String,
}

impl SqliteScheduleStore {
    /// Build a store over the sqlite file at `db_path`. Does not touch the
    /// database — call [`Self::init_schema`] before first use.
    pub fn new(db_path: impl Into<String>) -> Self {
        Self {
            db_path: db_path.into(),
        }
    }

    /// Create the `schedules` table/indexes if absent. Idempotent (every
    /// statement is `IF NOT EXISTS`); safe to call on every startup.
    pub async fn init_schema(&self) -> anyhow::Result<()> {
        let path = self.db_path.clone();
        spawn_blocking(move || {
            let conn = open_conn(&path)?;
            conn.execute_batch(SCHEMA_SQL)?;
            Ok::<_, anyhow::Error>(())
        })
        .await?
    }

    /// Persist a new schedule (US-205).
    pub async fn add(&self, spec: &ScheduleSpec) -> anyhow::Result<()> {
        let path = self.db_path.clone();
        let id = spec.id.clone();
        let owner = spec.owner.clone();
        let intent = spec.intent.clone();
        let kind_json = serde_json::to_string(&spec.kind)?;
        let missed_json = serde_json::to_string(&spec.missed)?;
        let tz = spec.tz.clone();
        let next_fire_nanos = spec.next_fire_nanos;
        let revoked = spec.revoked as i64;
        let revision = spec.revision as i64;
        spawn_blocking(move || {
            let conn = open_conn(&path)?;
            conn.execute(
                "INSERT INTO schedules
                    (id, owner_id, intent, kind_json, missed_json, tz, next_fire_nanos, revoked, revision)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                rusqlite::params![
                    id,
                    owner,
                    intent,
                    kind_json,
                    missed_json,
                    tz,
                    next_fire_nanos,
                    revoked,
                    revision,
                ],
            )?;
            Ok::<_, anyhow::Error>(())
        })
        .await?
    }

    /// List every schedule owned by `owner` (revoked included), soonest first.
    pub async fn list_for_owner(&self, owner: &str) -> anyhow::Result<Vec<ScheduleSpec>> {
        let path = self.db_path.clone();
        let owner = owner.to_string();
        spawn_blocking(move || {
            let conn = open_conn(&path)?;
            let mut stmt = conn.prepare(&format!(
                "SELECT {READ_COLUMNS} FROM schedules WHERE owner_id = ?1 \
                 ORDER BY next_fire_nanos ASC"
            ))?;
            let raws = stmt
                .query_map(rusqlite::params![owner], read_spec_row)?
                .collect::<Result<Vec<_>, _>>()?;
            let specs = raws
                .into_iter()
                .map(raw_to_spec)
                .collect::<anyhow::Result<Vec<ScheduleSpec>>>()?;
            Ok::<_, anyhow::Error>(specs)
        })
        .await?
    }

    /// Revoke a schedule owned by `owner`. Owner-scoped IDOR guard: bails on
    /// zero rows changed (wrong owner or missing id) — never silently no-ops.
    pub async fn revoke(&self, owner: &str, id: &str) -> anyhow::Result<()> {
        let path = self.db_path.clone();
        let owner = owner.to_string();
        let id = id.to_string();
        spawn_blocking(move || {
            let conn = open_conn(&path)?;
            let changed = conn.execute(
                "UPDATE schedules SET revoked = 1, revision = revision + 1 \
                 WHERE id = ?1 AND owner_id = ?2",
                rusqlite::params![id, owner],
            )?;
            if changed == 0 {
                anyhow::bail!(
                    "revoke: no schedule matched id/owner — wrong owner or missing id (IDOR guard)"
                );
            }
            Ok::<_, anyhow::Error>(())
        })
        .await?
    }

    /// Every non-revoked schedule due at `now_nanos` (`next_fire_nanos <=
    /// now`), soonest first. NOT owner-scoped — this is the firing loop's
    /// global sweep across all owners.
    pub async fn due(&self, now_nanos: i64) -> anyhow::Result<Vec<ScheduleSpec>> {
        let path = self.db_path.clone();
        spawn_blocking(move || {
            let conn = open_conn(&path)?;
            let mut stmt = conn.prepare(&format!(
                "SELECT {READ_COLUMNS} FROM schedules \
                 WHERE revoked = 0 AND next_fire_nanos <= ?1 ORDER BY next_fire_nanos ASC"
            ))?;
            let raws = stmt
                .query_map(rusqlite::params![now_nanos], read_spec_row)?
                .collect::<Result<Vec<_>, _>>()?;
            let specs = raws
                .into_iter()
                .map(raw_to_spec)
                .collect::<anyhow::Result<Vec<ScheduleSpec>>>()?;
            Ok::<_, anyhow::Error>(specs)
        })
        .await?
    }

    /// Advance a schedule's `next_fire_nanos` under optimistic concurrency
    /// (US-205). Owner-scoped IDOR guard + OCC on `expected_revision`; bails
    /// on zero rows changed (stale revision, wrong owner, or missing id).
    /// Returns the new revision.
    pub async fn set_next_fire(
        &self,
        owner: &str,
        id: &str,
        next_nanos: i64,
        expected_revision: u64,
    ) -> anyhow::Result<u64> {
        let path = self.db_path.clone();
        let owner = owner.to_string();
        let id = id.to_string();
        let expected = expected_revision as i64;
        let new_revision = expected_revision.saturating_add(1) as i64;
        spawn_blocking(move || {
            let conn = open_conn(&path)?;
            let changed = conn.execute(
                "UPDATE schedules SET next_fire_nanos = ?1, revision = ?2 \
                 WHERE id = ?3 AND owner_id = ?4 AND revision = ?5",
                rusqlite::params![next_nanos, new_revision, id, owner, expected],
            )?;
            if changed == 0 {
                anyhow::bail!(
                    "set_next_fire: no row matched id/owner/revision={expected} — stale \
                     expected_revision, wrong owner, or missing schedule (OCC guard)"
                );
            }
            Ok::<_, anyhow::Error>(new_revision as u64)
        })
        .await?
    }
}

/// Run the durable firing loop until dropped (US-205).
///
/// Ticks every `tick` (with [`MissedTickBehavior::Skip`], so a slow tick does
/// not stampede a backlog of ticks). Each tick sweeps [`SqliteScheduleStore::due`]
/// and, per due schedule, asks [`plan_fire`] how many times to invoke
/// `on_fire` and what the schedule's next fire should be, then advances:
/// `None` → [`SqliteScheduleStore::revoke`]; `Some(next)` →
/// [`SqliteScheduleStore::set_next_fire`] under the schedule's current
/// revision (OCC).
///
/// `on_fire` is the daemon-supplied dispatch callback — it decides what a fire
/// *does* (the store/loop stay decoupled from the task store, runtime registry
/// and proactive-turn queue). Store errors are logged and the loop continues;
/// they are retried on the next tick.
pub async fn run_scheduler<F, Fut>(store: Arc<SqliteScheduleStore>, tick: Duration, on_fire: F)
where
    F: Fn(ScheduleSpec) -> Fut,
    Fut: std::future::Future<Output = ()>,
{
    let mut ticker = interval(tick);
    ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);
    loop {
        ticker.tick().await;
        let now = now_nanos();
        let due = match store.due(now).await {
            Ok(due) => due,
            Err(e) => {
                tracing::warn!(
                    target: "bastion::schedule",
                    event = "due_query_failed",
                    error = %e,
                );
                continue;
            }
        };
        for spec in due {
            let plan = plan_fire(&spec.kind, &spec.missed, spec.next_fire_nanos, now);
            for _ in 0..plan.fire_count {
                on_fire(spec.clone()).await;
            }
            match plan.next_fire_nanos {
                None => {
                    if let Err(e) = store.revoke(&spec.owner, &spec.id).await {
                        tracing::warn!(
                            target: "bastion::schedule",
                            event = "revoke_failed",
                            schedule = %spec.id,
                            owner = %spec.owner,
                            error = %e,
                        );
                    }
                }
                Some(next) => {
                    if let Err(e) = store
                        .set_next_fire(&spec.owner, &spec.id, next, spec.revision)
                        .await
                    {
                        tracing::warn!(
                            target: "bastion::schedule",
                            event = "advance_failed",
                            schedule = %spec.id,
                            owner = %spec.owner,
                            error = %e,
                        );
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::NamedTempFile;

    async fn make_store() -> (NamedTempFile, SqliteScheduleStore) {
        let f = NamedTempFile::new().expect("tempfile");
        let path = f.path().to_str().expect("utf8 path").to_owned();
        let store = SqliteScheduleStore::new(path);
        store.init_schema().await.expect("init_schema");
        (f, store)
    }

    fn sample(id: &str, owner: &str, next_fire_nanos: i64) -> ScheduleSpec {
        ScheduleSpec {
            id: id.to_string(),
            owner: owner.to_string(),
            intent: "check the site".to_string(),
            kind: ScheduleKind::Every { interval_secs: 60 },
            missed: MissedPolicy::Skip,
            tz: None,
            next_fire_nanos,
            revoked: false,
            revision: 1,
        }
    }

    #[tokio::test]
    async fn add_list_revoke_round_trip_and_owner_isolation() {
        let (_f, store) = make_store().await;
        store.add(&sample("s1", "alice", 1_000)).await.expect("add");

        let alice = store.list_for_owner("alice").await.expect("list");
        assert_eq!(alice.len(), 1);
        assert_eq!(alice[0].id, "s1");
        assert_eq!(alice[0].kind, ScheduleKind::Every { interval_secs: 60 });

        // Wrong owner sees nothing.
        assert!(store
            .list_for_owner("bob")
            .await
            .expect("list bob")
            .is_empty());

        // Wrong-owner revoke bails (IDOR guard).
        assert!(store.revoke("bob", "s1").await.is_err());

        // Correct-owner revoke succeeds and flips the flag.
        store.revoke("alice", "s1").await.expect("revoke");
        let after = store.list_for_owner("alice").await.expect("relist");
        assert_eq!(after.len(), 1);
        assert!(after[0].revoked);
    }

    #[tokio::test]
    async fn due_returns_only_past_due_non_revoked() {
        let (_f, store) = make_store().await;
        store.add(&sample("past", "alice", 100)).await.expect("add");
        store
            .add(&sample("future", "alice", 10_000))
            .await
            .expect("add");
        let mut revoked = sample("revoked", "alice", 50);
        revoked.revoked = true;
        store.add(&revoked).await.expect("add");

        let due = store.due(1_000).await.expect("due");
        let ids: Vec<_> = due.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(ids, vec!["past"]);
    }

    #[test]
    fn compute_next_fire_semantics() {
        assert_eq!(
            compute_next_fire(&ScheduleKind::OneShot { at_nanos: 5 }, 5),
            None
        );
        assert_eq!(
            compute_next_fire(&ScheduleKind::Every { interval_secs: 2 }, 1_000),
            Some(1_000 + 2 * 1_000_000_000)
        );
        // Degenerate zero interval collapses to one-shot (no next).
        assert_eq!(
            compute_next_fire(&ScheduleKind::Every { interval_secs: 0 }, 1_000),
            None
        );
    }

    #[test]
    fn plan_fire_one_shot_revokes() {
        let plan = plan_fire(
            &ScheduleKind::OneShot { at_nanos: 100 },
            &MissedPolicy::Skip,
            100,
            200,
        );
        assert_eq!(
            plan,
            FirePlan {
                fire_count: 1,
                next_fire_nanos: None,
            }
        );
    }

    #[test]
    fn plan_fire_missed_policies() {
        let sec = 1_000_000_000i64;
        let kind = ScheduleKind::Every { interval_secs: 1 };
        // next_fire = 0, now = 5s => 6 slots due (0..=5s), overdue by >1 interval.
        let now = 5 * sec;

        let skip = plan_fire(&kind, &MissedPolicy::Skip, 0, now);
        assert_eq!(skip.fire_count, 0, "Skip drops an overdue backlog");
        assert_eq!(skip.next_fire_nanos, Some(6 * sec));

        let once = plan_fire(&kind, &MissedPolicy::RunOnce, 0, now);
        assert_eq!(once.fire_count, 1, "RunOnce collapses backlog to one fire");
        assert_eq!(once.next_fire_nanos, Some(6 * sec));

        let bounded = plan_fire(&kind, &MissedPolicy::CatchUpBounded { max: 3 }, 0, now);
        assert_eq!(bounded.fire_count, 3, "CatchUpBounded caps at max");
        assert_eq!(bounded.next_fire_nanos, Some(6 * sec));

        // On-time (exactly one slot due): every policy fires once.
        let on_time = plan_fire(&kind, &MissedPolicy::Skip, 0, sec / 2);
        assert_eq!(on_time.fire_count, 1);
        assert_eq!(on_time.next_fire_nanos, Some(sec));
    }

    #[tokio::test]
    async fn set_next_fire_occ_rejects_stale_revision() {
        let (_f, store) = make_store().await;
        store.add(&sample("s1", "alice", 100)).await.expect("add");

        let new_rev = store
            .set_next_fire("alice", "s1", 500, 1)
            .await
            .expect("first advance");
        assert_eq!(new_rev, 2);

        // Reusing the now-stale revision (1) must bail.
        assert!(store.set_next_fire("alice", "s1", 900, 1).await.is_err());

        // Wrong owner must also bail.
        assert!(store.set_next_fire("bob", "s1", 900, 2).await.is_err());
    }
}
