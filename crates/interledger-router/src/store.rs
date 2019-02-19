use bytes::Bytes;
use interledger_service::AccountId;
use std::sync::Arc;

pub trait RouterStore: Clone + Send + Sync + 'static {
    // TODO this should return a slice instead of a Bytes to avoid having so many Arcs
    fn get_routing_table(&self) -> Arc<Vec<(Bytes, AccountId)>>;
}
