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
pub use self::server::create_server;
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
