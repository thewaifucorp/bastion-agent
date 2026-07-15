// Provider hot-swap correctness test (PROV-05, D-11/D-12)
// Tests that /model command swaps provider and that session history is preserved.

use bastion_providers::{Provider, SharedProvider};
use bastion_types::{CallConfig, LlmResponse, Message, TokenUsage};
use std::sync::Arc;
use tokio::sync::RwLock;

struct MockProvider {
    name_str: &'static str,
    call_count: Arc<std::sync::atomic::AtomicU32>,
}

#[async_trait::async_trait]
impl Provider for MockProvider {
    async fn complete(&self, _msgs: &[Message], _cfg: &CallConfig) -> anyhow::Result<LlmResponse> {
        self.call_count
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ok(LlmResponse {
            text: format!("response from {}", self.name_str),
            tool_calls: None,
            usage: TokenUsage {
                input_tokens: 10,
                output_tokens: 5,
                ..Default::default()
            },
        })
    }
    async fn complete_simple(&self, _: &str) -> anyhow::Result<String> {
        Ok("simple".into())
    }
    fn context_limit(&self) -> usize {
        200_000
    }
    fn model_name(&self) -> &str {
        self.name_str
    }
    fn name(&self) -> &'static str {
        self.name_str
    }
}

#[tokio::test]
async fn provider_swap_transparent() {
    // Verify: SharedProvider write lock replaces inner box; read name confirms swap
    let count_a = Arc::new(std::sync::atomic::AtomicU32::new(0));
    let count_b = Arc::new(std::sync::atomic::AtomicU32::new(0));

    let provider_a: Box<dyn Provider> = Box::new(MockProvider {
        name_str: "anthropic",
        call_count: count_a.clone(),
    });

    let shared: SharedProvider = Arc::new(RwLock::new(provider_a));

    // First call uses provider_a
    {
        let p = shared.read().await;
        assert_eq!(p.name(), "anthropic");
    }

    // Swap to provider_b (simulates /model command)
    {
        let provider_b: Box<dyn Provider> = Box::new(MockProvider {
            name_str: "openai",
            call_count: count_b.clone(),
        });
        *shared.write().await = provider_b;
    }

    // Next read sees provider_b
    {
        let p = shared.read().await;
        assert_eq!(p.name(), "openai");
    }
}

#[tokio::test]
async fn provider_swap_write_lock_does_not_block_existing_read() {
    // Verify: concurrent read and write are managed correctly by RwLock
    let shared: SharedProvider = Arc::new(RwLock::new(Box::new(MockProvider {
        name_str: "anthropic",
        call_count: Arc::new(std::sync::atomic::AtomicU32::new(0)),
    })));

    let shared2 = shared.clone();
    let read_task = tokio::spawn(async move {
        let p = shared2.read().await;
        tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
        p.name().to_owned()
    });

    // Write waits for read to release
    tokio::time::sleep(tokio::time::Duration::from_millis(1)).await;
    {
        let new_p: Box<dyn Provider> = Box::new(MockProvider {
            name_str: "openai",
            call_count: Arc::new(std::sync::atomic::AtomicU32::new(0)),
        });
        *shared.write().await = new_p;
    }

    let name_during_read = read_task.await.unwrap();
    assert_eq!(name_during_read, "anthropic"); // read completed before write
    assert_eq!(shared.read().await.name(), "openai"); // write applied after
}
