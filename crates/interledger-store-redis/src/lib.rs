//! # interledger-store-redis
//!
//! A Store that uses [Redis](https://redis.io/) as the database for storing account details, balances, the routing table, etc.
#[macro_use]
extern crate log;
#[macro_use]
extern crate serde;

mod account;
mod store;

pub use account::Account;
pub use store::{connect, connect_with_poll_interval, RedisStore};
