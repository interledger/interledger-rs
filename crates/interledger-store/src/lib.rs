//! # interledger-store
//!
//! Backend databases for storing account details, balances, the routing table, etc.

/// A module to define the primitive `Account` struct which implements `Account` related traits.
pub mod account;
/// Cryptographic utilities for encrypting/decrypting data as well as clearing data from memory
pub mod crypto;
/// A redis backend using [redis-rs](https://github.com/mitsuhiko/redis-rs/)
#[cfg(feature = "redis")]
pub mod redis;
