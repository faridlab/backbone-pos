// This directory's extension (hand-written; ADR-0031). The generated mod.rs (lib.rs for
// src/) pulls it in with include!, so its lines resolve against this directory.

/// Stable integration surface for composing services (the deliberate public API consumers depend on).
pub mod integration;
// The hand-owned request-pool shim (the composing service's tenant pool
// resolution): a generated-tree declaration the regenerator drops, so it
// lives in the preserved block (#447 cause-2 class).
pub mod request_pool;
