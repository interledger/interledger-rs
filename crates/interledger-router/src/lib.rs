//! # interledger-router
//!
//! A service that routes ILP Prepare packets to the correct next
//! account based on the ILP address in the Prepare packet based
//! on the routing table.
//!
//! A routing table could be as simple as a single entry for the empty prefix
//! ("") that will route all requests to a specific outgoing account.
//!
//! Note that the Router is not responsible for building the routing table,
//! only using the information provided by the store. The routing table in the
//! store can either be configured or populated using the `CcpRouteManager`
//! (see the `interledger-ccp` crate for more details).

use bytes::Bytes;
use interledger_service::{Account, AccountStore};
use std::collections::HashMap;

mod router;

pub use self::router::Router;

/// A trait for Store implmentations that have ILP routing tables.
pub trait RouterStore: AccountStore + Clone + Send + Sync + 'static {
    /// **Synchronously** return a copy of the routing table.
    /// Note that this is synchronous because it assumes that Stores should
    /// keep the routing table in memory and use PubSub or polling to keep it updated.
    /// This ensures that individual packets can be routed without hitting the underlying store.
    // TODO avoid using HashMap because it means it'll be cloned a lot
    fn routing_table(&self) -> HashMap<Bytes, <Self::Account as Account>::AccountId>;
}
