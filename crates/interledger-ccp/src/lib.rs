//! # interledger-ccp
//!
//! This crate implements the Connector-to-Connector Protocol (CCP) for exchanging routing
//! information with peers. The `CcpRouteManager` processes Route Update and Route Control
//! messages from accounts that we are configured to receive routes from and sends route
//! updates to accounts that we are configured to send updates to.
//!
//! The `CcpRouteManager` writes changes to the routing table to the store so that the
//! updates are used by the `Router` to forward incoming packets to the best next hop
//! we know about.

use futures::Future;
pub use interledger_service::RoutingRelation;
use interledger_service::{Account, AccountId};
use std::collections::HashMap;
use std::{fmt, str::FromStr};

#[cfg(test)]
mod fixtures;
mod packet;
mod routing_table;
mod server;
//#[cfg(test)]
//mod test_helpers;

pub use packet::{Mode, RouteControlRequest};
pub use server::{CcpRouteManager, CcpRouteManagerBuilder};

use serde::{Deserialize, Serialize};

// key = Bytes, key should be Address -- TODO
type Routes<T> = HashMap<String, T>;
type LocalAndConfiguredRoutes<T> = (Routes<T>, Routes<T>);

pub trait RouteManagerStore: Clone {
    // TODO should we have a way to only get the details for specific routes?
    fn get_local_and_configured_routes(
        &self,
    ) -> Box<dyn Future<Item = LocalAndConfiguredRoutes<Account>, Error = ()> + Send>;

    fn get_accounts_to_send_routes_to(
        &self,
        ignore_accounts: Vec<AccountId>,
    ) -> Box<dyn Future<Item = Vec<Account>, Error = ()> + Send>;

    fn get_accounts_to_receive_routes_from(
        &self,
    ) -> Box<dyn Future<Item = Vec<Account>, Error = ()> + Send>;

    fn set_routes(
        &mut self,
        routes: impl IntoIterator<Item = (String, Account)>,
    ) -> Box<dyn Future<Item = (), Error = ()> + Send>;
}
