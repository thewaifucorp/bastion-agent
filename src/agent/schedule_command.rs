//! US-205 — the `/schedule` cockpit: create, list and revoke durable
//! schedules. Owner-scoped, reused by every surface (like `/task`), and — as
//! it needs the schedule store rather than `&mut AgentLoop` — special-cased in
//! the daemon dispatch rather than routed through the generic `CommandHandler`.

use std::sync::Arc;

use crate::adaptive::schedule::{
    now_nanos, MissedPolicy, ScheduleKind, ScheduleSpec, SqliteScheduleStore,
};

/// Handle `/schedule <sub> [args]`. `arg` is everything after `/schedule`.
pub async fn handle(
    store: &Arc<SqliteScheduleStore>,
    arg: Option<&str>,
    owner: &str,
) -> anyhow::Result<String> {
    let arg = arg.unwrap_or("").trim();
    let (sub, rest) = match arg.split_once(char::is_whitespace) {
        Some((s, r)) => (s, r.trim()),
        None => (arg, ""),
    };
    match sub {
        "" | "list" => list(store, owner).await,
        "add" => add(store, owner, rest).await,
        "cancel" | "revoke" => cancel(store, owner, rest).await,
        other => Ok(format!(
            "unknown /schedule subcommand '{other}'. Use: list | add every <secs> <intent> | \
             add once <secs> <intent> | cancel <id>"
        )),
    }
}

async fn list(store: &Arc<SqliteScheduleStore>, owner: &str) -> anyhow::Result<String> {
    let specs = store.list_for_owner(owner).await?;
    if specs.is_empty() {
        return Ok("no schedules.".to_string());
    }
    let mut out = String::from("schedules:\n");
    for s in &specs {
        let kind = match &s.kind {
            ScheduleKind::OneShot { .. } => "once".to_string(),
            ScheduleKind::Every { interval_secs } => format!("every {interval_secs}s"),
        };
        let state = if s.revoked { "revoked" } else { "active" };
        out.push_str(&format!("  {}  [{kind}, {state}]  {}\n", s.id, s.intent));
    }
    Ok(out.trim_end().to_string())
}

async fn add(store: &Arc<SqliteScheduleStore>, owner: &str, rest: &str) -> anyhow::Result<String> {
    // `add <every|once> <secs> <intent...>`
    let usage = "usage: /schedule add every <secs> <intent>  |  /schedule add once <secs> <intent>";
    let (mode, tail) = match rest.split_once(char::is_whitespace) {
        Some(p) => p,
        None => return Ok(usage.to_string()),
    };
    let (secs_str, intent) = match tail.trim().split_once(char::is_whitespace) {
        Some((s, i)) if !i.trim().is_empty() => (s, i.trim()),
        _ => return Ok(usage.to_string()),
    };
    let secs: u64 = match secs_str.parse() {
        Ok(n) => n,
        Err(_) => return Ok(format!("'{secs_str}' is not a whole number of seconds.")),
    };
    let now = now_nanos();
    let (kind, next_fire) = match mode {
        "every" => (
            ScheduleKind::Every {
                interval_secs: secs,
            },
            now.saturating_add((secs as i64).saturating_mul(1_000_000_000)),
        ),
        "once" => {
            let at = now.saturating_add((secs as i64).saturating_mul(1_000_000_000));
            (ScheduleKind::OneShot { at_nanos: at }, at)
        }
        _ => return Ok(usage.to_string()),
    };
    let spec = ScheduleSpec {
        id: format!("sched-{now}"),
        owner: owner.to_string(),
        intent: intent.to_string(),
        kind,
        missed: MissedPolicy::Skip,
        tz: None,
        next_fire_nanos: next_fire,
        revoked: false,
        revision: 1,
    };
    let id = spec.id.clone();
    store.add(&spec).await?;
    Ok(format!("scheduled {id}: {mode} {secs}s → {intent}"))
}

async fn cancel(store: &Arc<SqliteScheduleStore>, owner: &str, id: &str) -> anyhow::Result<String> {
    if id.is_empty() {
        return Ok("usage: /schedule cancel <id>".to_string());
    }
    match store.revoke(owner, id).await {
        Ok(_) => Ok(format!("schedule {id} cancelled.")),
        Err(e) => Ok(format!("cannot cancel schedule {id}: {e}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::NamedTempFile;

    async fn store() -> (NamedTempFile, Arc<SqliteScheduleStore>) {
        let f = NamedTempFile::new().unwrap();
        let s = SqliteScheduleStore::new(f.path().to_str().unwrap());
        s.init_schema().await.unwrap();
        (f, Arc::new(s))
    }

    #[tokio::test]
    async fn add_list_cancel_round_trip() {
        let (_f, s) = store().await;
        let out = handle(&s, Some("add every 3600 check the site"), "alice")
            .await
            .unwrap();
        assert!(out.contains("scheduled"));
        let listed = handle(&s, Some("list"), "alice").await.unwrap();
        assert!(listed.contains("check the site"));
        assert!(listed.contains("every 3600s"));

        // wrong owner sees nothing
        let bob = handle(&s, Some("list"), "bob").await.unwrap();
        assert_eq!(bob, "no schedules.");
    }

    #[tokio::test]
    async fn add_rejects_bad_seconds() {
        let (_f, s) = store().await;
        let out = handle(&s, Some("add every soon do it"), "alice")
            .await
            .unwrap();
        assert!(out.contains("not a whole number"));
    }
}
