use super::fixtures::*;
use super::redis_helpers::*;
use env_logger;
use futures::Future;
use interledger_api::NodeStore;
use interledger_store_redis::{RedisStore, RedisStoreBuilder};
use lazy_static::lazy_static;
use parking_lot::Mutex;
use tokio::runtime::Runtime;

lazy_static! {
    static ref TEST_MUTEX: Mutex<()> = Mutex::new(());
}

pub fn test_store() -> impl Future<Item = (RedisStore, TestContext), Error = ()> {
    let context = TestContext::new();
    RedisStoreBuilder::new(context.get_client_connection_info(), [0; 32])
        .connect()
        .and_then(|store| {
            let store_clone = store.clone();
            store
                .clone()
                .insert_account(ACCOUNT_DETAILS_0.clone())
                .and_then(move |_| store_clone.insert_account(ACCOUNT_DETAILS_1.clone()))
                .and_then(|_| Ok((store, context)))
        })
}

// same as test_store, but uses an account without `settle_to` for netting tests.
pub fn test_other_store() -> impl Future<Item = (RedisStore, TestContext), Error = ()> {
    let context = TestContext::new();
    RedisStoreBuilder::new(context.get_client_connection_info(), [0; 32])
        .connect()
        .and_then(|store| {
            let store_clone = store.clone();
            store
                .clone()
                .insert_account(ACCOUNT_DETAILS_0.clone())
                .and_then(move |_| store_clone.insert_account(ACCOUNT_DETAILS_3.clone()))
                .and_then(|_| Ok((store, context)))
        })
}

pub fn block_on<F>(f: F) -> Result<F::Item, F::Error>
where
    F: Future + Send + 'static,
    F::Item: Send,
    F::Error: Send,
{
    // Only run one test at a time
    let _ = env_logger::try_init();
    let lock = TEST_MUTEX.lock();
    let mut runtime = Runtime::new().unwrap();
    let result = runtime.block_on(f);
    drop(lock);
    result
}
