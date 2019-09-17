//! # interledger-store
//!
//! Stores account details, balances, the routing table, etc.
//! Supported backends:  
//! 1. [Redis](https://redis.io/)

mod backends;
mod store;
pub(crate) mod utils;
pub use store::*;

pub use utils::account::{Account, AccountId};

#[cfg(feature = "redis")]
pub use backends::redis::{RedisStore, RedisStoreBuilder};
#[cfg(feature = "redis")]
pub use redis_crate::{ConnectionInfo, IntoConnectionInfo};
