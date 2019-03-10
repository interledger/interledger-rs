//! # interledger-store-memory
//!
//! A simple in-memory store intended primarily for testing and
//! stateless sender/receiver services that are passed all of the
//! relevant account details when the store is instantiated.

mod account;
mod store;

pub use self::account::{Account, AccountBuilder};
pub use self::store::InMemoryStore;
