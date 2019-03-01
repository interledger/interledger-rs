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
mod service;

pub use self::client::{connect_client, parse_btp_url};
pub use self::service::BtpService;

pub trait BtpAccount: Account {
    fn get_btp_url(&self) -> Option<&Url>;
}

pub trait BtpStore {
    type Account: BtpAccount;

    fn get_account_from_token(
        &self,
        token: &str,
    ) -> Box<Future<Item = Self::Account, Error = ()> + Send>;
}
