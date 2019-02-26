use bytes::Bytes;
use futures::Future;
use interledger_service::AccountId;

pub struct AccountDetails {
    pub client_address: Bytes,
    pub asset_scale: u8,
    pub asset_code: String,
}

pub trait IldcpStore {
    fn get_account_details(
        &self,
        account_id: AccountId,
    ) -> Box<Future<Item = AccountDetails, Error = ()> + Send>;
}
