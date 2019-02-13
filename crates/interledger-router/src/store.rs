use bytes::Bytes;
use futures::Future;
use interledger_service::AccountId;

pub trait RouterStore: Clone + Send + Sync + 'static {
    fn get_next_hop(
        &self,
        destination: &[u8],
    ) -> Box<Future<Item = (AccountId, Bytes), Error = ()> + Send>;
}
