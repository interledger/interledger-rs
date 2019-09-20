//! # interledger-ildcp
//!
//! Client and server implementations of the [Interledger Dynamic Configuration Protocol (ILDCP)](https://github.com/interledger/rfcs/blob/master/0031-dynamic-configuration-protocol/0031-dynamic-configuration-protocol.md).
//!
//! This is used by clients to query for their ILP address and asset details such as asset code and scale.

use interledger_service::Account;

mod client;
mod packet;
mod server;

pub use client::get_ildcp_info;
pub use packet::*;
pub use server::IldcpService;
