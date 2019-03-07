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
pub use self::server::{create_server, create_open_signup_server};
pub use self::service::BtpService;

pub trait BtpAccount: Account {
    fn get_btp_uri(&self) -> Option<&Url>;
}

pub trait BtpStore {
    type Account: BtpAccount;

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

pub trait BtpOpenSignupStore {
    type Account: BtpAccount;

    fn create_btp_account<'a>(
        &self,
        account: BtpOpenSignupAccount<'a>,
    ) -> Box<Future<Item = Self::Account, Error = ()> + Send>;
}
