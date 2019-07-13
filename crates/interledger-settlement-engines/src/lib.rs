#![recursion_limit = "128"]
#[macro_use]
extern crate log;

#[macro_use]
#[cfg(test)]
extern crate lazy_static;

#[cfg(test)]
extern crate env_logger;

#[cfg(test)]
extern crate serde_json;

#[macro_use]
extern crate tower_web;

extern crate ethabi;

use futures::Future;

// Export all the engines
mod engines;
pub use self::engines::ethereum_ledger::{
    EthereumAccount, EthereumAddresses, EthereumLedgerSettlementEngine, EthereumLedgerTxSigner,
    EthereumStore,
};
pub use ethereum_tx_sign::web3::types::Address as EthAddress;

mod api;
pub use self::api::SettlementEngineApi;

use hyper::Response;
use interledger_settlement::SettlementData;

/// TODO: Docs. Trait to abstract over engines at the top level HTTP rest api.
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
