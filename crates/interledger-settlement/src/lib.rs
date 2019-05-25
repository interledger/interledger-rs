#[macro_use]
extern crate log;

use interledger_service::Account;
use url::Url;

mod client;
mod message_handler;

pub use client::SettlementClient;
pub use message_handler::SettlementMessageService;

pub trait SettlementAccount: Account {
    fn settlement_engine_url(&self) -> Option<Url> {
        None
    }

    fn settlement_engine_asset_scale(&self) -> Option<u8> {
        None
    }
}
