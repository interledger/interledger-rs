//! # Interledger Settlement Engines
//!
//! Crate containing all the components for implementing the Settlement
//! Architecture for the Interledger Protocol. The crate is structured such that
//! an API is created by giving it an object which implements the
//! SettlementEngine trait. All settlement engines must be implemented under the
//! `engines` subdirectory, with a directory name describing their
//! functionality, e.g. ethereum_ledger, ethereum_unidirectional_channel,
//! xrp_ledger, etc.
#![recursion_limit = "128"]

#[macro_use]
extern crate tower_web;

use futures::Future;

// Export all the engines
mod engines;
pub use self::engines::ethereum_ledger::{
    EthereumAccount, EthereumAddresses, EthereumLedgerSettlementEngine, EthereumLedgerTxSigner,
    EthereumStore,
};
mod stores;
pub use self::stores::redis_ethereum_ledger::{
    EthereumLedgerRedisStore, EthereumLedgerRedisStoreBuilder,
};
pub use ethereum_tx_sign::web3::types::Address as EthAddress;

mod api;
pub use self::api::SettlementEngineApi;

use hyper::Response;
use interledger_settlement::SettlementData;

/// Trait consumed by the Settlement Engine HTTP API. Every settlement engine
/// MUST implement this trait, so that it can be then be exposed over the API.
pub trait SettlementEngine {
    fn send_money(
        &self,
        account_id: String,
        money: SettlementData,
        idempotency_key: Option<String>,
    ) -> Box<dyn Future<Item = Response<String>, Error = Response<String>> + Send>;

    fn receive_message(
        &self,
        account_id: String,
        message: Vec<u8>,
        idempotency_key: Option<String>,
    ) -> Box<dyn Future<Item = Response<String>, Error = Response<String>> + Send>;

    fn create_account(
        &self,
        account_id: String,
        idempotency_key: Option<String>,
    ) -> Box<dyn Future<Item = Response<String>, Error = Response<String>> + Send>;
}
