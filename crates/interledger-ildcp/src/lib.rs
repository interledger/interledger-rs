#[macro_use]
extern crate lazy_static;
#[macro_use]
extern crate log;

use bytes::Bytes;
use interledger_service::Account;

mod client;
mod packet;
mod server;

pub use client::get_ildcp_info;
pub use packet::*;
pub use server::IldcpService;

// TODO this should return borrowed values, but the IldcpResponseBuilder
// complained that the values did not live long enough
pub trait IldcpAccount: Account {
    fn client_address(&self) -> Bytes;
    fn asset_scale(&self) -> u8;
    fn asset_code(&self) -> String;
}
