//! Cursor pagination for `/v1/*` list endpoints (US — External Control Plane
//! and SDK, Phase 2).
//!
//! `bastion_runtime::task::TaskStore` has no native cursor/limit support
//! (`list_cases_for_owner`/`list_attempts_for_case` return everything for the
//! owner/case in one call — confirmed against the pinned dependency; see
//! `docs/en/control-plane-security.md`). Extending that trait is out of scope
//! for a read-only-routes phase (it's a `bastion-core` change), so pagination
//! here is an honest app-layer slice over an already-fully-fetched,
//! deterministically sorted `Vec`, not a real `LIMIT`/`OFFSET` or keyset query
//! — fine at the scale a single-owner personal-agent deployment produces, but
//! worth knowing if this ever needs to scale to a store with thousands of
//! cases per owner.

use base64::Engine;

/// Sort key every paginated list in this module orders by: a primary
/// timestamp (nanoseconds since epoch) descending, with the item's own id as
/// a deterministic tiebreaker for items sharing a timestamp (see
/// `control_plane::credential`'s identical tiebreaker rationale).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct SortKey<'a> {
    // Ord on a tuple of (Reverse(ts), Reverse(id)) would also work, but the
    // explicit descending comparator below is easier to read at the call site.
    ts: i64,
    id: &'a str,
}

fn cmp_desc(a: &SortKey, b: &SortKey) -> std::cmp::Ordering {
    b.ts.cmp(&a.ts).then_with(|| b.id.cmp(a.id))
}

/// Opaque cursor: base64url(ts:id). Not meant to be constructed by a client —
/// only round-tripped from a `next_cursor` this module emitted.
pub fn encode_cursor(ts: i64, id: &str) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(format!("{ts}:{id}"))
}

fn decode_cursor(cursor: &str) -> Option<(i64, String)> {
    let decoded = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(cursor)
        .ok()?;
    let s = String::from_utf8(decoded).ok()?;
    let (ts, id) = s.split_once(':')?;
    Some((ts.parse().ok()?, id.to_string()))
}

/// One page of `items` (already sorted (ts DESC, id DESC) by the caller),
/// keyed by `(ts, id)` extracted via `key_of`. Returns the page and, if more
/// items remain past it, a `next_cursor` for the next call.
///
/// A malformed/unrecognized `cursor` (never issued by this module, or stale
/// against data that changed) is treated as "start from the beginning" —
/// fail-open on pagination position (never on auth/scope) is the right
/// default here: the worst case is a client re-sees the first page, not a
/// data leak.
pub fn paginate<T>(
    mut items: Vec<T>,
    key_of: impl Fn(&T) -> (i64, &str),
    cursor: Option<&str>,
    page_size: usize,
) -> (Vec<T>, Option<String>) {
    items.sort_by(|a, b| {
        let (a_ts, a_id) = key_of(a);
        let (b_ts, b_id) = key_of(b);
        cmp_desc(
            &SortKey { ts: a_ts, id: a_id },
            &SortKey { ts: b_ts, id: b_id },
        )
    });

    let start = match cursor.and_then(decode_cursor) {
        Some((cursor_ts, cursor_id)) => items
            .iter()
            .position(|item| {
                let (ts, id) = key_of(item);
                cmp_desc(
                    &SortKey { ts, id },
                    &SortKey {
                        ts: cursor_ts,
                        id: &cursor_id,
                    },
                ) == std::cmp::Ordering::Greater
            })
            .unwrap_or(items.len()),
        None => 0,
    };

    let remaining = &items[start..];
    let page_len = remaining.len().min(page_size);
    let has_more = remaining.len() > page_len;
    let next_cursor = if has_more {
        let (ts, id) = key_of(&remaining[page_len - 1]);
        Some(encode_cursor(ts, id))
    } else {
        None
    };

    let page = items.drain(start..start + page_len).collect();
    (page, next_cursor)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, Clone, PartialEq)]
    struct Item {
        id: String,
        ts: i64,
    }

    fn item(id: &str, ts: i64) -> Item {
        Item {
            id: id.to_string(),
            ts,
        }
    }

    fn key_of(i: &Item) -> (i64, &str) {
        (i.ts, i.id.as_str())
    }

    #[test]
    fn first_page_sorted_newest_first() {
        let items = vec![item("a", 100), item("b", 300), item("c", 200)];
        let (page, next) = paginate(items, key_of, None, 10);
        assert_eq!(
            page.iter().map(|i| i.id.as_str()).collect::<Vec<_>>(),
            vec!["b", "c", "a"]
        );
        assert!(next.is_none(), "everything fit in one page");
    }

    #[test]
    fn pagination_across_two_pages_covers_everything_exactly_once() {
        let items = vec![
            item("a", 100),
            item("b", 300),
            item("c", 200),
            item("d", 400),
            item("e", 50),
        ];
        let (page1, cursor1) = paginate(items.clone(), key_of, None, 2);
        assert_eq!(
            page1.iter().map(|i| i.id.as_str()).collect::<Vec<_>>(),
            vec!["d", "b"]
        );
        let cursor1 = cursor1.expect("more pages remain");

        let (page2, cursor2) = paginate(items.clone(), key_of, Some(&cursor1), 2);
        assert_eq!(
            page2.iter().map(|i| i.id.as_str()).collect::<Vec<_>>(),
            vec!["c", "a"]
        );
        let cursor2 = cursor2.expect("one more page remains");

        let (page3, cursor3) = paginate(items, key_of, Some(&cursor2), 2);
        assert_eq!(
            page3.iter().map(|i| i.id.as_str()).collect::<Vec<_>>(),
            vec!["e"]
        );
        assert!(cursor3.is_none(), "no more pages after the last item");
    }

    #[test]
    fn same_timestamp_items_are_ordered_deterministically_by_id() {
        let items = vec![item("b", 100), item("a", 100), item("c", 100)];
        let (page, _) = paginate(items, key_of, None, 10);
        // ts DESC ties broken by id DESC — same rule every time, regardless
        // of input order.
        assert_eq!(
            page.iter().map(|i| i.id.as_str()).collect::<Vec<_>>(),
            vec!["c", "b", "a"]
        );
    }

    #[test]
    fn malformed_cursor_fails_open_to_page_one() {
        let items = vec![item("a", 100), item("b", 200)];
        let (page, _) = paginate(items, key_of, Some("not-a-real-cursor"), 10);
        assert_eq!(
            page.len(),
            2,
            "an unrecognized cursor starts over, not empty/error"
        );
    }

    #[test]
    fn empty_input_yields_empty_page_and_no_cursor() {
        let items: Vec<Item> = vec![];
        let (page, next) = paginate(items, key_of, None, 10);
        assert!(page.is_empty());
        assert!(next.is_none());
    }
}
