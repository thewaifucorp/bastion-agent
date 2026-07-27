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
             add once <secs> <intent> | add daily <HH:MM[±HH:MM]> <intent> | cancel <id>"
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
            ScheduleKind::DailyAt {
                hour,
                minute,
                offset_minutes,
            } => format!(
                "daily {hour:02}:{minute:02}{}",
                format_offset(*offset_minutes)
            ),
        };
        let state = if s.revoked { "revoked" } else { "active" };
        out.push_str(&format!("  {}  [{kind}, {state}]  {}\n", s.id, s.intent));
    }
    Ok(out.trim_end().to_string())
}

/// Renders a fixed UTC offset the same way it is typed: `Z` for UTC,
/// `±HH:MM` otherwise.
fn format_offset(offset_minutes: i32) -> String {
    if offset_minutes == 0 {
        return "Z".to_string();
    }
    let sign = if offset_minutes < 0 { '-' } else { '+' };
    let abs = offset_minutes.abs();
    format!("{sign}{:02}:{:02}", abs / 60, abs % 60)
}

/// Parses `HH:MM`, `HH:MMZ` and `HH:MM±HH:MM` into
/// `(hour, minute, offset_minutes)`.
///
/// A bare `HH:MM` means UTC, NOT the daemon host's local zone: the host's
/// offset is invisible to the person typing the command (and can differ
/// between the console and a channel), so guessing it would make the same
/// input mean different instants on different deployments. The offset is
/// explicit or it is UTC.
fn parse_daily_time(token: &str) -> Option<(u32, u32, i32)> {
    let (clock, offset) = match token.strip_suffix(['Z', 'z']) {
        Some(clock) => (clock, 0),
        None => match token.find(['+', '-']) {
            // `find` from the start would split `-03:00` in a token that is
            // ONLY an offset; a clock part is always `H:MM`/`HH:MM`, so the
            // separator can never be earlier than index 3.
            Some(idx) if idx >= 3 => (&token[..idx], parse_offset(&token[idx..])?),
            Some(_) => return None,
            None => (token, 0),
        },
    };
    let (h, m) = clock.split_once(':')?;
    let hour: u32 = h.parse().ok()?;
    let minute: u32 = m.parse().ok()?;
    if hour > 23 || minute > 59 {
        return None;
    }
    Some((hour, minute, offset))
}

/// `±HH:MM` or `±HH` into signed minutes east of UTC.
fn parse_offset(raw: &str) -> Option<i32> {
    let (sign, rest) = match raw.as_bytes().first()? {
        b'+' => (1, &raw[1..]),
        b'-' => (-1, &raw[1..]),
        _ => return None,
    };
    let (h, m) = match rest.split_once(':') {
        Some((h, m)) => (h, m),
        None => (rest, "0"),
    };
    let hours: i32 = h.parse().ok()?;
    let minutes: i32 = m.parse().ok()?;
    if hours > 23 || minutes > 59 {
        return None;
    }
    Some(sign * (hours * 60 + minutes))
}

async fn add(store: &Arc<SqliteScheduleStore>, owner: &str, rest: &str) -> anyhow::Result<String> {
    // `add <every|once> <secs> <intent...>` | `add daily <HH:MM[±HH:MM]> <intent...>`
    let usage = "usage: /schedule add every <secs> <intent>  |  /schedule add once <secs> <intent>  \
                 |  /schedule add daily <HH:MM[±HH:MM]> <intent>";
    let (mode, tail) = match rest.split_once(char::is_whitespace) {
        Some(p) => p,
        None => return Ok(usage.to_string()),
    };
    let (head, intent) = match tail.trim().split_once(char::is_whitespace) {
        Some((s, i)) if !i.trim().is_empty() => (s, i.trim()),
        _ => return Ok(usage.to_string()),
    };
    let now = now_nanos();

    if mode == "daily" {
        let Some((hour, minute, offset_minutes)) = parse_daily_time(head) else {
            return Ok(format!(
                "'{head}' is not a wall-clock time. Use HH:MM (UTC), HH:MMZ, or HH:MM±HH:MM."
            ));
        };
        let kind = ScheduleKind::DailyAt {
            hour,
            minute,
            offset_minutes,
        };
        let Some(next_fire) =
            crate::adaptive::schedule::next_daily_fire(hour, minute, offset_minutes, now)
        else {
            return Ok(format!("cannot compute a fire time for '{head}'."));
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
        return Ok(format!(
            "scheduled {id}: daily {hour:02}:{minute:02}{} → {intent}",
            format_offset(offset_minutes)
        ));
    }

    let secs_str = head;
    let secs: u64 = match secs_str.parse() {
        Ok(n) => n,
        Err(_) => return Ok(format!("'{secs_str}' is not a whole number of seconds.")),
    };
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

    #[test]
    fn a_bare_clock_time_means_utc_not_the_host_zone() {
        assert_eq!(parse_daily_time("09:00"), Some((9, 0, 0)));
        assert_eq!(parse_daily_time("09:00Z"), Some((9, 0, 0)));
    }

    #[test]
    fn an_explicit_offset_is_parsed_with_its_sign() {
        assert_eq!(parse_daily_time("09:00-03:00"), Some((9, 0, -180)));
        assert_eq!(parse_daily_time("09:00+05:30"), Some((9, 0, 330)));
        assert_eq!(parse_daily_time("23:59-08"), Some((23, 59, -480)));
    }

    #[test]
    fn out_of_range_clock_or_offset_is_refused() {
        assert_eq!(parse_daily_time("24:00"), None);
        assert_eq!(parse_daily_time("09:60"), None);
        assert_eq!(parse_daily_time("09:00+24:00"), None);
        assert_eq!(parse_daily_time("0900"), None);
        assert_eq!(parse_daily_time(""), None);
        // A token that is only an offset has no clock part to take.
        assert_eq!(parse_daily_time("-03:00"), None);
    }

    #[test]
    fn offsets_render_the_way_they_are_typed() {
        assert_eq!(format_offset(0), "Z");
        assert_eq!(format_offset(-180), "-03:00");
        assert_eq!(format_offset(330), "+05:30");
    }

    #[tokio::test]
    async fn add_daily_round_trips_with_its_offset() {
        let (_f, s) = store().await;
        let out = handle(&s, Some("add daily 09:00-03:00 morning review"), "alice")
            .await
            .unwrap();
        assert!(out.contains("daily 09:00-03:00"), "{out}");

        let listed = handle(&s, Some("list"), "alice").await.unwrap();
        assert!(listed.contains("daily 09:00-03:00"), "{listed}");
        assert!(listed.contains("morning review"));
    }

    #[tokio::test]
    async fn add_daily_rejects_a_non_time_head() {
        let (_f, s) = store().await;
        let out = handle(&s, Some("add daily tomorrow do it"), "alice")
            .await
            .unwrap();
        assert!(out.contains("not a wall-clock time"), "{out}");
    }
}
