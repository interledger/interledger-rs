use futures::Future;
use interledger_service::AccountId;

pub struct HttpDetails {
    pub url: String,
    pub auth_header: String,
}

// TODO do we need all of these constraints?
pub trait HttpStore: Clone + Send + Sync + 'static {
    fn get_account_from_authorization(
        &self,
        auth_header: &str,
    ) -> Box<Future<Item = AccountId, Error = ()> + Send>;
    fn get_http_details_for_account(
        &self,
        account: AccountId,
    ) -> Box<Future<Item = HttpDetails, Error = ()> + Send>;
}
