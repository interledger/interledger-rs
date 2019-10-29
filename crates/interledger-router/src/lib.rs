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

use interledger_service::{Account, AccountStore};
use std::{collections::HashMap, sync::Arc};

mod router;

pub use self::router::Router;

/// A trait for Store implmentations that have ILP routing tables.
pub trait RouterStore: AccountStore + Clone + Send + Sync + 'static {
    /// **Synchronously** return the routing table.
    /// Note that this is synchronous because it assumes that Stores should
    /// keep the routing table in memory and use PubSub or polling to keep it updated.
    /// This ensures that individual packets can be routed without hitting the underlying store.
    /// An Arc is returned to avoid copying the underlying data while processing each packet.
    fn routing_table(&self) -> Arc<HashMap<String, <Self::Account as Account>::AccountId>>;
}
