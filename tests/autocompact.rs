use bastion_runtime::agent::compactor::AutoCompact;
use bastion_types::{Message, MessageContent, Role};

fn make_messages(n: usize) -> Vec<Message> {
    (0..n)
        .map(|i| Message {
            role: if i % 2 == 0 {
                Role::User
            } else {
                Role::Assistant
            },
            content: MessageContent::Text(format!("message {}", i)),
        })
        .collect()
}

#[test]
fn needs_compaction_at_threshold() {
    let ac = AutoCompact::new();
    assert!(ac.needs_compaction(160_000, 200_000)); // exactly 80%
    assert!(ac.needs_compaction(180_000, 200_000)); // over
}

#[test]
fn needs_compaction_below_threshold() {
    let ac = AutoCompact::new();
    assert!(!ac.needs_compaction(159_999, 200_000)); // 79.999%
    assert!(!ac.needs_compaction(0, 200_000));
}

#[test]
fn needs_compaction_zero_limit() {
    let ac = AutoCompact::new();
    assert!(!ac.needs_compaction(1000, 0)); // defensive: zero limit → no compact
}

#[tokio::test]
async fn compact_skips_when_few_messages() {
    use bastion_providers::Provider;
    use bastion_types::{CallConfig, LlmResponse};

    struct MockProvider;
    #[async_trait::async_trait]
    impl Provider for MockProvider {
        async fn complete(&self, _: &[Message], _: &CallConfig) -> anyhow::Result<LlmResponse> {
            panic!("should not be called")
        }
        async fn complete_simple(&self, _: &str) -> anyhow::Result<String> {
            panic!("should not be called")
        }
        fn context_limit(&self) -> usize {
            200_000
        }
        fn model_name(&self) -> &str {
            "mock"
        }
        fn name(&self) -> &'static str {
            "mock"
        }
    }

    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("t.db").to_str().unwrap().to_owned();
    let sm = bastion_runtime::session::SessionManager::new(&db);
    sm.init_schema().await.unwrap();
    let sid = sm.create_session().await.unwrap();

    let ac = AutoCompact::new(); // keep_last = 20
    let msgs = make_messages(15); // fewer than 20 — no compact
    let result: Vec<_> = ac
        .compact(
            &sid,
            &msgs,
            &MockProvider as &dyn bastion_providers::Provider,
            &sm,
        )
        .await
        .unwrap();
    assert_eq!(result.len(), 15); // unchanged
}

#[tokio::test]
async fn compact_fires_with_enough_messages() {
    use bastion_providers::Provider;
    use bastion_types::{CallConfig, LlmResponse};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

    struct MockProvider {
        called: Arc<AtomicBool>,
    }
    #[async_trait::async_trait]
    impl Provider for MockProvider {
        async fn complete(&self, _: &[Message], _: &CallConfig) -> anyhow::Result<LlmResponse> {
            unreachable!()
        }
        async fn complete_simple(&self, _: &str) -> anyhow::Result<String> {
            self.called.store(true, Ordering::SeqCst);
            Ok("Summary of older messages".to_owned())
        }
        fn context_limit(&self) -> usize {
            200_000
        }
        fn model_name(&self) -> &str {
            "mock"
        }
        fn name(&self) -> &'static str {
            "mock"
        }
    }

    let called = Arc::new(AtomicBool::new(false));
    let called2 = called.clone();

    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("t.db").to_str().unwrap().to_owned();
    let sm = bastion_runtime::session::SessionManager::new(&db);
    sm.init_schema().await.unwrap();
    let sid = sm.create_session().await.unwrap();

    let ac = AutoCompact::new();
    let msgs = make_messages(25);
    let mock = MockProvider { called: called2 };
    let result: Vec<_> = ac
        .compact(&sid, &msgs, &mock as &dyn bastion_providers::Provider, &sm)
        .await
        .unwrap();

    assert!(
        called.load(Ordering::SeqCst),
        "complete_simple must be called"
    );
    // Result: 1 summary sentinel + 20 recent messages
    assert_eq!(result.len(), 21);
    assert_eq!(result[0].role, Role::System);
}
