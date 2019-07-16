use super::redis_helpers::*;
use super::store::{EthereumLedgerRedisStore, EthereumLedgerRedisStoreBuilder};
use env_logger;
use futures::Future;
use tokio::runtime::Runtime;

pub fn test_store() -> impl Future<Item = (EthereumLedgerRedisStore, TestContext), Error = ()> {
    let context = TestContext::new();
    EthereumLedgerRedisStoreBuilder::new(context.get_client_connection_info())
        .connect()
        .and_then(|store| Ok((store, context)))
}

pub fn block_on<F>(f: F) -> Result<F::Item, F::Error>
where
    F: Future + Send + 'static,
    F::Item: Send,
    F::Error: Send,
{
    // Only run one test at a time
    let _ = env_logger::try_init();
    let mut runtime = Runtime::new().unwrap();
    runtime.block_on(f)
}
