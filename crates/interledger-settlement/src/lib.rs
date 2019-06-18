#![recursion_limit = "128"]

#[macro_use]
extern crate log;
#[macro_use]
extern crate tower_web;

use futures::Future;
use interledger_service::Account;
use interledger_packet::Address;
use url::Url;

mod api;
mod client;
mod message_service;

pub use api::SettlementApi;
pub use client::SettlementClient;
pub use message_service::SettlementMessageService;

pub struct SettlementEngineDetails {
    /// Base URL of the settlement engine
    pub url: Url,
    /// Asset scale that the settlement engine is configured to use.
    /// For example, sending a settlement for amount 1000 to a settlement engine
    /// that uses as scale of 3 would mean that it should send 1 whole unit of that asset.
    /// The SettlementClient translates the amounts used for each account internally within
    /// Interledger.rs into the correct scale used by the settlement engine.
    pub asset_scale: u8,
    /// The ILP address of the settlement engine. For example, `peer.settle.xrp-paychan`.
    /// Note that both peers' settlement engines are expected to use the same address.
    pub ilp_address: Address,
}

pub trait SettlementAccount: Account {
    fn settlement_engine_details(&self) -> Option<SettlementEngineDetails> {
        None
    }
}

pub trait SettlementStore {
    type Account: SettlementAccount;

    fn update_balance_for_incoming_settlement(
        &self,
        account_id: <Self::Account as Account>::AccountId,
        amount: u64,
    ) -> Box<Future<Item = (), Error = ()> + Send>;
}
