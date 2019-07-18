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
pub mod engines;
pub mod stores;
use self::stores::redis_store;
mod api;
pub use self::api::SettlementEngineApi;

#[derive(Extract, Debug, Clone, Copy)]
pub struct Quantity {
    amount: u64,
}

use http::StatusCode;

pub type ApiResponse = (StatusCode, String);

/// Trait consumed by the Settlement Engine HTTP API. Every settlement engine
/// MUST implement this trait, so that it can be then be exposed over the API.
pub trait SettlementEngine {
    fn send_money(
        &self,
        account_id: String,
        money: Quantity,
    ) -> Box<dyn Future<Item = ApiResponse, Error = ApiResponse> + Send>;

    fn receive_message(
        &self,
        account_id: String,
        message: Vec<u8>,
    ) -> Box<dyn Future<Item = ApiResponse, Error = ApiResponse> + Send>;

    fn create_account(
        &self,
        account_id: String,
    ) -> Box<dyn Future<Item = ApiResponse, Error = ApiResponse> + Send>;
}
