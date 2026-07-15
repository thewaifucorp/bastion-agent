use bastion_runtime::session::SessionManager;
use bastion_types::{Message, MessageContent, Role};

#[tokio::test]
async fn messages_survive_restart() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("test.db").to_str().unwrap().to_owned();

    let sm = SessionManager::new(&db);
    sm.init_schema().await.unwrap();
    let sid = sm.create_session().await.unwrap();

    sm.append(
        &sid,
        Message {
            role: Role::User,
            content: MessageContent::Text("hello".into()),
        },
        None,
    )
    .await
    .unwrap();
    sm.append(
        &sid,
        Message {
            role: Role::Assistant,
            content: MessageContent::Text("hi".into()),
        },
        Some(42),
    )
    .await
    .unwrap();

    // Simulate restart
    let sm2 = SessionManager::new(&db);
    let msgs = sm2.load_recent(&sid).await.unwrap();
    assert_eq!(msgs.len(), 2);
    assert_eq!(msgs[0].role, Role::User);
    assert_eq!(msgs[1].role, Role::Assistant);
}

#[tokio::test]
async fn orphaned_tool_result_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("test.db").to_str().unwrap().to_owned();
    let sm = SessionManager::new(&db);
    sm.init_schema().await.unwrap();
    let sid = sm.create_session().await.unwrap();
    sm.append(
        &sid,
        Message {
            role: Role::User,
            content: MessageContent::Text("q".into()),
        },
        None,
    )
    .await
    .unwrap();

    // Try to append Tool without preceding Assistant — must fail
    let result = sm
        .append(
            &sid,
            Message {
                role: Role::Tool,
                content: MessageContent::Text("result".into()),
            },
            None,
        )
        .await;
    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(
        err.contains("Orphaned") || err.contains("orphaned") || err.contains("tool_use"),
        "got: {}",
        err
    );
}

/// Ciclo 2.1 fix (`docs/revamp/C2-approval-port-design.md` §3): a round with
/// MULTIPLE `tool_calls` appends one Tool-role row per call, sequentially —
/// `dispatch_tool_loop` has always done this (e.g. the DenyScope::Turn skip
/// path pairs a denied tool call with a synthetic "skipped" Tool result for
/// every OTHER tool call in the same round). The second (and later) Tool row
/// in that sequence is preceded by another Tool row, not the Assistant
/// message — this must NOT be rejected as orphaned.
#[tokio::test]
async fn tool_after_tool_within_the_same_round_is_accepted() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("test.db").to_str().unwrap().to_owned();
    let sm = SessionManager::new(&db);
    sm.init_schema().await.unwrap();
    let sid = sm.create_session().await.unwrap();

    sm.append(
        &sid,
        Message {
            role: Role::Assistant,
            content: MessageContent::Text("calling two tools".into()),
        },
        None,
    )
    .await
    .expect("assistant message");

    sm.append(
        &sid,
        Message {
            role: Role::Tool,
            content: MessageContent::Text("first tool result".into()),
        },
        None,
    )
    .await
    .expect("first Tool row, preceded by Assistant, must be accepted");

    sm.append(
        &sid,
        Message {
            role: Role::Tool,
            content: MessageContent::Text("second tool result".into()),
        },
        None,
    )
    .await
    .expect(
        "second Tool row in the SAME round, preceded by another Tool row, \
         must be accepted — not rejected as orphaned",
    );

    let msgs = sm.load_recent(&sid).await.unwrap();
    assert_eq!(msgs.len(), 3);
    assert_eq!(msgs[0].role, Role::Assistant);
    assert_eq!(msgs[1].role, Role::Tool);
    assert_eq!(msgs[2].role, Role::Tool);
}

/// Companion negative check: the fix above widens the accepted preceding
/// role to include `Tool`, but Tool-after-User (or after nothing) must still
/// be rejected — unchanged from `orphaned_tool_result_rejected`.
#[tokio::test]
async fn tool_after_user_is_still_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("test.db").to_str().unwrap().to_owned();
    let sm = SessionManager::new(&db);
    sm.init_schema().await.unwrap();
    let sid = sm.create_session().await.unwrap();

    sm.append(
        &sid,
        Message {
            role: Role::User,
            content: MessageContent::Text("hi".into()),
        },
        None,
    )
    .await
    .unwrap();

    let result = sm
        .append(
            &sid,
            Message {
                role: Role::Tool,
                content: MessageContent::Text("result".into()),
            },
            None,
        )
        .await;
    assert!(
        result.is_err(),
        "Tool immediately after User (never after Assistant/Tool) must still be rejected"
    );
}

#[tokio::test]
async fn load_most_recent_id_works() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("test.db").to_str().unwrap().to_owned();
    let sm = SessionManager::new(&db);
    sm.init_schema().await.unwrap();
    assert!(sm.load_most_recent_id().await.unwrap().is_none());
    let sid = sm.create_session().await.unwrap();
    assert_eq!(sm.load_most_recent_id().await.unwrap(), Some(sid));
}

#[tokio::test]
async fn budget_check_enforced() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("test.db").to_str().unwrap().to_owned();
    let sm = SessionManager::new(&db);
    sm.init_schema().await.unwrap();
    assert!(sm.check_budget(5.0).await.unwrap()); // no spend yet → under budget
    sm.update_budget(4.99).await.unwrap();
    assert!(sm.check_budget(5.0).await.unwrap()); // still under
    sm.update_budget(0.02).await.unwrap();
    assert!(!sm.check_budget(5.0).await.unwrap()); // over budget
}
