//! # interledger-btp
//!
//! Client and server implementations of the [Bilateral Transport Protocol (BTP)](https://github.com/interledger/rfcs/blob/master/0023-bilateral-transfer-protocol/0023-bilateral-transfer-protocol.md).
//! This is a WebSocket-based protocol for exchanging ILP packets between directly connected peers.
//!
//! Because this protocol uses WebSockets, only one party needs to have a publicly-accessible HTTPS
//! endpoint but both sides can send and receive ILP packets.

#[macro_use]
extern crate quick_error;
#[cfg(test)]
#[macro_use]
extern crate lazy_static;
#[macro_use]
extern crate log;

use futures::Future;
use interledger_service::Account;
use url::Url;

mod client;
mod errors;
mod oer;
mod packet;
mod server;
mod service;

pub use self::client::{connect_client, parse_btp_url};
pub use self::server::{create_open_signup_server, create_server};
pub use self::service::{BtpOutgoingService, BtpService};

pub trait BtpAccount: Account {
    fn get_btp_uri(&self) -> Option<&Url>;
}

/// The interface for Store implementations that can be used with the BTP Server.
pub trait BtpStore {
    type Account: BtpAccount;

    /// Load Account details based on the auth token received via BTP.
    fn get_account_from_auth(
        &self,
        token: &str,
        username: Option<&str>,
    ) -> Box<Future<Item = Self::Account, Error = ()> + Send>;
}

pub struct BtpOpenSignupAccount<'a> {
    pub auth_token: &'a str,
    pub username: Option<&'a str>,
    pub ilp_address: &'a [u8],
    pub asset_code: &'a str,
    pub asset_scale: u8,
}

/// The interface for Store implementatoins that allow open BTP signups.
/// Every incoming WebSocket connection will automatically have a BtpOpenSignupAccount
/// created and added to the store.
///
/// **WARNING:** Users and store implementors should be careful when implementing this trait because
/// malicious users can use open signups to create very large numbers of accounts and
/// crash the process or fill up the database.
pub trait BtpOpenSignupStore {
    type Account: BtpAccount;

    fn create_btp_account<'a>(
        &self,
        account: BtpOpenSignupAccount<'a>,
    ) -> Box<Future<Item = Self::Account, Error = ()> + Send>;
}
