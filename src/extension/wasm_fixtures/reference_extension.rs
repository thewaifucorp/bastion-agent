//! Source for the reference `Wasm`-kind extension module
//! (`docs/revamp/C3-extension-protocol-design.md` §2/§6).
//!
//! NOT part of any cargo package — this file is compiled directly with
//! `rustc` (not `cargo`) into a freestanding `wasm32-unknown-unknown`
//! module, deliberately outside the workspace so it never affects the host
//! (x86_64) build graph, `check-crate-deps.sh`, or `dump-public-api.sh`.
//! `reference_extension.wasm` next to this file is the committed, prebuilt
//! output — regenerate it with:
//!
//! ```sh
//! rustc --edition 2021 --target wasm32-unknown-unknown --crate-type cdylib \
//!   -C opt-level=s -C panic=abort -C strip=symbols \
//!   -o src/extension/wasm_fixtures/reference_extension.wasm \
//!   src/extension/wasm_fixtures/reference_extension.rs
//! ```
//!
//! Exports (both `extern "C" fn(i64, i64) -> i64`, the ONLY calling
//! convention `bastion-extension-wasm`'s `WasmSandbox::call_i64_i64_to_i64`
//! understands — no string/JSON marshalling across the wasm boundary this
//! cycle, keeping the guest ABI trivial and auditable):
//! - `add`: deterministic, terminates immediately — proves a normal call
//!   round-trips through the sandbox correctly.
//! - `busy_loop`: never terminates — proves the sandbox's fuel budget
//!   actually bounds a hostile/buggy guest instead of hanging the daemon
//!   (exercised by the adversarial suite, `tests/extension_adversarial.rs`).
//!
//! Deliberately has ZERO imports (no WASI, no host functions of any kind) —
//! the module cannot reach a syscall even if it wanted to; there is nothing
//! for it to call. This is the strongest available proof of "no ambient
//! authority": not merely policy-denied, but structurally absent.
#![no_std]

use core::panic::PanicInfo;

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    core::arch::wasm32::unreachable()
}

#[no_mangle]
pub extern "C" fn add(a: i64, b: i64) -> i64 {
    a.wrapping_add(b)
}

#[no_mangle]
pub extern "C" fn busy_loop(_a: i64, _b: i64) -> i64 {
    let mut x: i64 = 0;
    loop {
        x = x.wrapping_add(1);
        // Volatile-ish side effect so the loop can't be const-folded away.
        core::hint::black_box(x);
    }
}
