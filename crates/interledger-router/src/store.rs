use bytes::Bytes;
use futures::Future;
use interledger_service::AccountId;
use std::sync::Arc;

pub trait RouterStore: Clone + Send + Sync + 'static {
    fn get_routing_table(&self) -> Arc<Vec<(&[u8], (AccountId, &[u8]))>>;
}
