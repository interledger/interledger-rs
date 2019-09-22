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

use bytes::Bytes;
use futures::Future;
use interledger_service::Account;
use std::collections::HashMap;
use std::{str::FromStr, string::ToString};

#[cfg(test)]
mod fixtures;
mod packet;
mod routing_table;
mod server;
#[cfg(test)]
mod test_helpers;

pub use packet::{Mode, RouteControlRequest};
pub use server::{CcpRouteManager, CcpRouteManagerBuilder};

use serde::{Deserialize, Serialize};

/// Data structure used to describe the routing relation of an account with its peers.
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, PartialOrd, Serialize, Deserialize)]
pub enum RoutingRelation {
    /// An account from which we do not receive routes from, neither broadcast
    /// routes to
    NonRoutingAccount = 0,
    /// An account from which we receive routes from, but do not broadcast
    /// routes to
    Parent = 1,
    /// An account from which we receive routes from and broadcast routes to
    Peer = 2,
    /// An account from which we do not receive routes from, but broadcast
    /// routes to
    Child = 3,
}

impl FromStr for RoutingRelation {
    type Err = ();

    fn from_str(string: &str) -> Result<Self, ()> {
        match string.to_lowercase().as_str() {
            "nonroutingaccount" => Ok(RoutingRelation::NonRoutingAccount),
            "parent" => Ok(RoutingRelation::Parent),
            "peer" => Ok(RoutingRelation::Peer),
            "child" => Ok(RoutingRelation::Child),
            _ => Err(()),
        }
    }
}

impl ToString for RoutingRelation {
    fn to_string(&self) -> String {
        match self {
            RoutingRelation::NonRoutingAccount => "NonRoutingAccount".to_string(),
            RoutingRelation::Parent => "Parent".to_string(),
            RoutingRelation::Peer => "Peer".to_string(),
            RoutingRelation::Child => "Child".to_string(),
        }
    }
}

/// Define CcpAccount methods and Account types that need to be used by the CCP Service
pub trait CcpRoutingAccount: Account {
    /// The type of relationship we have with this account
    fn routing_relation(&self) -> RoutingRelation;

    /// Indicates whether we should send CCP Route Updates to this account
    fn should_send_routes(&self) -> bool {
        self.routing_relation() == RoutingRelation::Child
            || self.routing_relation() == RoutingRelation::Peer
    }

    /// Indicates whether we should accept CCP Route Update Requests from this account
    fn should_receive_routes(&self) -> bool {
        self.routing_relation() == RoutingRelation::Parent
            || self.routing_relation() == RoutingRelation::Peer
    }
}

// key = Bytes, key should be Address -- TODO
type Route<T> = HashMap<Bytes, T>;
type LocalAndConfiguredRoutes<T> = (Route<T>, Route<T>);

pub trait RouteManagerStore: Clone {
    type Account: CcpRoutingAccount;

    // TODO should we have a way to only get the details for specific routes?
    fn get_local_and_configured_routes(
        &self,
    ) -> Box<dyn Future<Item = LocalAndConfiguredRoutes<Self::Account>, Error = ()> + Send>;

    fn get_accounts_to_send_routes_to(
        &self,
    ) -> Box<dyn Future<Item = Vec<Self::Account>, Error = ()> + Send>;

    fn get_accounts_to_receive_routes_from(
        &self,
    ) -> Box<dyn Future<Item = Vec<Self::Account>, Error = ()> + Send>;

    fn set_routes(
        &mut self,
        routes: impl IntoIterator<Item = (Bytes, Self::Account)>,
    ) -> Box<dyn Future<Item = (), Error = ()> + Send>;
}
