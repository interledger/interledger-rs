//! # interledger-store-redis
//!
//! A Store that uses [Redis](https://redis.io/) as the database for storing account details, balances, the routing table, etc.
#[macro_use]
extern crate log;

mod account;
mod store;

pub use account::Account;
pub use store::{connect, RedisStore};
