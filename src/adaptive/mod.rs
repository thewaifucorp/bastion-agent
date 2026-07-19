//! Adaptive Execution wiring for the Bastion Agent (US-201..208).
//!
//! Composes the neutral `bastion_runtime::task` mechanism (contract, store,
//! cycle, ports) into the product: the deterministic mode selector that picks
//! the smallest capable lifecycle for a request (US-201), and a concrete
//! `Observer` bridge so the kernel's neutral lifecycle events reach tracing
//! (the cycle needs one; the kernel ships only a no-op default).

pub mod mode;
pub mod observer;

pub use mode::{select_mode, ModeDecision, ModeSource};
pub use observer::TracingObserver;
