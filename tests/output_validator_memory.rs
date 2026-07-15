//! End-to-end OutputValidator tests moved VERBATIM from the kernel crate's
//! `hooks/output_validator.rs` unit-test module (M2 step 3b, decision A4):
//! they need the real `SqliteMemory` backend and `EvalFailureSink` (product),
//! which the kernel crate cannot depend on. Asserts are untouched.

use bastion_memory::SharedMemory;
use bastion_runtime::hooks::output_validator::OutputValidator;

// --- end-to-end: store → contest → revoked ---

#[tokio::test]
async fn contestation_soft_revokes_matching_belief() {
    use bastion_memory::sqlite::SqliteMemory;
    use bastion_runtime::session::sqlite::SessionManager;
    use std::sync::Arc;
    use tempfile::NamedTempFile;
    use tokio::sync::RwLock;

    // Setup temp DB
    let f = NamedTempFile::new().unwrap();
    let path = f.path().to_str().unwrap().to_owned();
    let session_mgr = SessionManager::new(&path);
    session_mgr.init_schema().await.expect("init_schema");

    let mem: SharedMemory = Arc::new(RwLock::new(Box::new(SqliteMemory::new(&path))));

    let owner = "user_test";

    // Store a belief about exercising
    let _id = {
        let m = mem.read().await;
        m.store_belief(
            owner,
            None,
            "Mario exercises every morning",
            "sess1",
            "user",
            false,
            None,
        )
        .await
        .expect("store_belief")
    };

    // Verify it is retrieved before contestation
    let before = {
        let m = mem.read().await;
        m.retrieve_tagged(owner, None)
            .await
            .expect("retrieve before")
    };
    assert_eq!(before.len(), 1, "belief should exist before contestation");

    // Contestation: user says the exercise belief is wrong
    let validator = OutputValidator::new(Arc::new(
        bastion_cognition::eval::failure_sink::EvalFailureSink,
    ));
    validator
        .validate(
            "isso não é mais verdade sobre exercises morning",
            &mem,
            owner,
        )
        .await
        .expect("validate");

    // After contestation: belief should be revoked (excluded from retrieve_tagged)
    let after = {
        let m = mem.read().await;
        m.retrieve_tagged(owner, None)
            .await
            .expect("retrieve after")
    };
    assert!(
        after.is_empty(),
        "belief must be excluded from retrieve_tagged after contestation"
    );
}

#[tokio::test]
async fn no_contestation_leaves_belief_intact() {
    use bastion_memory::sqlite::SqliteMemory;
    use bastion_runtime::session::sqlite::SessionManager;
    use std::sync::Arc;
    use tempfile::NamedTempFile;
    use tokio::sync::RwLock;

    let f = NamedTempFile::new().unwrap();
    let path = f.path().to_str().unwrap().to_owned();
    let session_mgr = SessionManager::new(&path);
    session_mgr.init_schema().await.expect("init_schema");

    let mem: SharedMemory = Arc::new(RwLock::new(Box::new(SqliteMemory::new(&path))));
    let owner = "user_test2";

    {
        let m = mem.read().await;
        m.store_belief(
            owner,
            None,
            "Mario likes coffee",
            "sess1",
            "user",
            false,
            None,
        )
        .await
        .expect("store");
    }

    let validator = OutputValidator::new(Arc::new(
        bastion_cognition::eval::failure_sink::EvalFailureSink,
    ));
    validator
        .validate("what's the weather today?", &mem, owner)
        .await
        .expect("validate no-op");

    let beliefs = {
        let m = mem.read().await;
        m.retrieve_tagged(owner, None).await.expect("retrieve")
    };
    assert_eq!(
        beliefs.len(),
        1,
        "belief must remain intact when no contestation"
    );
}
