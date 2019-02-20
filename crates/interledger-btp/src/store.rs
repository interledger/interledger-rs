use futures::Future;
use interledger_service::AccountId;
use url::Url;

pub trait BtpStore {
    fn get_btp_url(&self, account: &AccountId) -> Box<Future<Item = Url, Error = ()>>;
    fn get_account_from_token(&self, token: &str) -> Box<Future<Item = AccountId, Error = ()>>;
}
