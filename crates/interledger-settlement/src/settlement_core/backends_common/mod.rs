/// This module holds baseline implementations for the idempotency and leftover-related features
/// which should be shared across engines which use the same store backend. An engine backend's
/// imlpementation could directly use the provided store and not have to worry about any
/// idempotency or leftover-related functionality.
#[cfg(feature = "redis")]
pub mod redis;
