//! SQLite persistence for `/committee` decisions (outcome logging).
//!
//! A self-contained table (`committee_outcomes`) living in the SAME file as the
//! session DB (`cfg.session.db_path`), but owned entirely by bastion-agent, not
//! bastion-core — same precedent `SqliteTaskStore` already set (a second schema
//! owner sharing one DB file, own `init_schema`, own connection-per-call). No
//! bastion-core schema change needed for this half of the feature.
//!
//! `/committee <question>` (`committee::handle`) inserts a row after the
//! pipeline finishes. `/committee outcome <id> <helpful|harmful|neutral>`
//! (`committee::handle_outcome`) reads it back, applies the per-persona
//! stigmergy reinforcement/weaken (M6), and records the outcome.

use rusqlite::{Connection, OptionalExtension};
use tokio::task;

/// One signal persona's stage-1 vote plus whether it matched the candidate the
/// committee actually carried forward to Risk Manager / Portfolio Manager.
/// Unanimous stage 1 => `aligned: true` for every signal persona. A stage-2
/// debate => `aligned: true` only for personas NOT in the synthesized
/// `CabinetVerdict::dissents` (i.e. whichever position survived synthesis).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SignalRecord {
    pub persona: String,
    pub signal: String,
    pub aligned: bool,
}

#[derive(Debug, Clone)]
pub struct CommitteeOutcomeRow {
    pub id: i64,
    pub question: String,
    pub signals: Vec<SignalRecord>,
    pub final_recommendation: String,
    pub outcome: Option<String>,
}

fn now_nanos() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system clock before UNIX epoch")
        .as_nanos() as i64
}

pub async fn init_schema(db_path: &str) -> anyhow::Result<()> {
    let path = db_path.to_owned();
    task::spawn_blocking(move || {
        let conn = Connection::open(&path)?;
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA busy_timeout=5000;")?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS committee_outcomes (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                owner_id TEXT NOT NULL,
                question TEXT NOT NULL,
                signals_json TEXT NOT NULL,
                final_recommendation TEXT NOT NULL,
                created_at INTEGER NOT NULL,
                outcome TEXT,
                outcome_recorded_at INTEGER
             );
             CREATE INDEX IF NOT EXISTS idx_committee_outcomes_owner \
                ON committee_outcomes(owner_id);",
        )?;
        Ok::<(), anyhow::Error>(())
    })
    .await?
}

/// Insert a new row for a just-finished `/committee` run. Returns the new row id
/// (surfaced to the operator so they can later reference it in `/committee
/// outcome <id> ...`).
pub async fn insert(
    db_path: &str,
    owner_id: &str,
    question: &str,
    signals: &[SignalRecord],
    final_recommendation: &str,
) -> anyhow::Result<i64> {
    let path = db_path.to_owned();
    let owner_id = owner_id.to_owned();
    let question = question.to_owned();
    let signals_json = serde_json::to_string(signals)?;
    let final_recommendation = final_recommendation.to_owned();
    task::spawn_blocking(move || {
        let conn = Connection::open(&path)?;
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA busy_timeout=5000;")?;
        conn.execute(
            "INSERT INTO committee_outcomes \
                (owner_id, question, signals_json, final_recommendation, created_at) \
             VALUES (?1, ?2, ?3, ?4, ?5)",
            rusqlite::params![
                owner_id,
                question,
                signals_json,
                final_recommendation,
                now_nanos()
            ],
        )?;
        Ok::<i64, anyhow::Error>(conn.last_insert_rowid())
    })
    .await?
}

/// Owner-scoped lookup (IDOR guard): a wrong owner sees `Ok(None)`, indistinguishable
/// from a missing id.
pub async fn get(
    db_path: &str,
    owner_id: &str,
    id: i64,
) -> anyhow::Result<Option<CommitteeOutcomeRow>> {
    let path = db_path.to_owned();
    let owner_id = owner_id.to_owned();
    task::spawn_blocking(move || {
        let conn = Connection::open(&path)?;
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA busy_timeout=5000;")?;
        let row = conn
            .query_row(
                "SELECT id, question, signals_json, final_recommendation, outcome \
                 FROM committee_outcomes WHERE id = ?1 AND owner_id = ?2",
                rusqlite::params![id, owner_id],
                |row| {
                    let signals_json: String = row.get(2)?;
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, String>(1)?,
                        signals_json,
                        row.get::<_, String>(3)?,
                        row.get::<_, Option<String>>(4)?,
                    ))
                },
            )
            .optional()?;
        Ok::<_, anyhow::Error>(row.map(
            |(id, question, signals_json, final_recommendation, outcome)| {
                let signals: Vec<SignalRecord> =
                    serde_json::from_str(&signals_json).unwrap_or_default();
                CommitteeOutcomeRow {
                    id,
                    question,
                    signals,
                    final_recommendation,
                    outcome,
                }
            },
        ))
    })
    .await?
}

/// Owner-scoped (IDOR guard): errors when no row matches (id, owner_id), same
/// discipline as `Memory::revoke_belief` — a wrong owner cannot silently no-op.
/// Errors (instead of silently overwriting) when the outcome was already recorded,
/// since M6's stigmergy adjustment must never double-apply for the same row.
pub async fn record_outcome(
    db_path: &str,
    owner_id: &str,
    id: i64,
    outcome: &str,
) -> anyhow::Result<()> {
    let path = db_path.to_owned();
    let owner_id = owner_id.to_owned();
    let outcome = outcome.to_owned();
    task::spawn_blocking(move || {
        let conn = Connection::open(&path)?;
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA busy_timeout=5000;")?;
        let changed = conn.execute(
            "UPDATE committee_outcomes SET outcome = ?4, outcome_recorded_at = ?5 \
             WHERE id = ?1 AND owner_id = ?2 AND outcome IS NULL",
            rusqlite::params![id, owner_id, outcome, outcome, now_nanos()],
        )?;
        if changed == 0 {
            anyhow::bail!(
                "registro #{id} não encontrado para este owner, ou já tem um resultado registrado"
            );
        }
        Ok::<(), anyhow::Error>(())
    })
    .await?
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn temp_db() -> (tempfile::TempDir, String) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir
            .path()
            .join("committee.db")
            .to_str()
            .unwrap()
            .to_string();
        init_schema(&path).await.unwrap();
        (dir, path)
    }

    #[tokio::test]
    async fn insert_then_get_round_trips() {
        let (_dir, db_path) = temp_db().await;
        let signals = vec![
            SignalRecord {
                persona: "fundamentalist".into(),
                signal: "BUY".into(),
                aligned: true,
            },
            SignalRecord {
                persona: "contrarian".into(),
                signal: "BUY".into(),
                aligned: true,
            },
        ];
        let id = insert(&db_path, "alice", "AAPL?", &signals, "comprar 1%")
            .await
            .expect("insert");

        let row = get(&db_path, "alice", id)
            .await
            .expect("get")
            .expect("row present");
        assert_eq!(row.question, "AAPL?");
        assert_eq!(row.final_recommendation, "comprar 1%");
        assert_eq!(row.signals.len(), 2);
        assert!(row.outcome.is_none());
    }

    #[tokio::test]
    async fn get_is_owner_scoped() {
        let (_dir, db_path) = temp_db().await;
        let id = insert(&db_path, "alice", "q", &[], "r").await.unwrap();

        let bob_view = get(&db_path, "bob", id).await.expect("get");
        assert!(
            bob_view.is_none(),
            "a different owner must not see alice's row"
        );
    }

    #[tokio::test]
    async fn record_outcome_then_reject_double_record() {
        let (_dir, db_path) = temp_db().await;
        let id = insert(&db_path, "alice", "q", &[], "r").await.unwrap();

        record_outcome(&db_path, "alice", id, "helpful")
            .await
            .expect("first record");
        let row = get(&db_path, "alice", id).await.unwrap().unwrap();
        assert_eq!(row.outcome.as_deref(), Some("helpful"));

        let second = record_outcome(&db_path, "alice", id, "harmful").await;
        assert!(
            second.is_err(),
            "recording an outcome twice must error, not overwrite"
        );
    }

    #[tokio::test]
    async fn record_outcome_rejects_cross_owner_id() {
        let (_dir, db_path) = temp_db().await;
        let id = insert(&db_path, "alice", "q", &[], "r").await.unwrap();

        let res = record_outcome(&db_path, "bob", id, "helpful").await;
        assert!(
            res.is_err(),
            "bob must not be able to record an outcome on alice's row"
        );

        let row = get(&db_path, "alice", id).await.unwrap().unwrap();
        assert!(
            row.outcome.is_none(),
            "alice's row must be untouched by bob's rejected call"
        );
    }
}
