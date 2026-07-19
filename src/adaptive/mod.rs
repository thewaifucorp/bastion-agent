//! Adaptive Execution wiring for the Bastion Agent (US-201..208).
//!
//! Composes the neutral `bastion_runtime::task` mechanism (contract, store,
//! cycle, ports) into the product: the deterministic mode selector that picks
//! the smallest capable lifecycle for a request (US-201), a concrete
//! `Observer` bridge so the kernel's neutral lifecycle events reach tracing
//! (the cycle needs one; the kernel ships only a no-op default), and the
//! `TaskExecutor`/`Chooser`/`Verifier` port implementations that let a durable
//! `Pursue` coding task actually run by delegating to a registered external
//! `AgentRuntime` (US-203/206, `exec`).

pub mod delegate;
pub mod enqueue;
pub mod exec;
pub mod mode;
pub mod observer;
pub mod schedule;

pub use delegate::{decompose, run_delegated};
pub use enqueue::enqueue_pursue;
pub use exec::{coding_cycle, CodingChooser, RuntimeOutcomeVerifier, RuntimeTaskExecutor};
pub use mode::{select_mode, ModeDecision, ModeSource};
pub use observer::TracingObserver;
pub use schedule::{
    compute_next_fire, plan_fire, run_scheduler, FirePlan, MissedPolicy, ScheduleKind,
    ScheduleSpec, SqliteScheduleStore,
};
