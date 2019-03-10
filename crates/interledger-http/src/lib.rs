#[macro_use]
extern crate log;

use futures::Future;
use interledger_service::Account;
use url::Url;

mod client;
mod server;

pub use self::client::HttpClientService;
pub use self::server::HttpServerService;

pub trait HttpAccount: Account {
    fn get_http_url(&self) -> Option<&Url>;
    fn get_http_auth_header(&self) -> Option<&str>;
}

// TODO do we need all of these constraints?
pub trait HttpStore: Clone + Send + Sync + 'static {
    type Account: HttpAccount;

    fn get_account_from_authorization(
        &self,
        auth_header: &str,
    ) -> Box<Future<Item = Self::Account, Error = ()> + Send>;
}
