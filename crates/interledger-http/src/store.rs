use futures::Future;
use interledger_service::AccountId;

pub struct HttpDetails {
    pub url: String,
    pub auth_header: String,
}

pub trait HttpStore {
    fn get_account_from_authorization(
        &self,
        auth_header: &str,
    ) -> Box<Future<Item = AccountId, Error = ()>>;
    fn get_http_details_for_account(
        &self,
        account: AccountId,
    ) -> Box<Future<Item = HttpDetails, Error = ()>>;
}
