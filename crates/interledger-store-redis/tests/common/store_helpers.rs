use super::fixtures::*;
use super::redis_helpers::*;
use env_logger;
use futures::Future;
use interledger_api::NodeStore;
use interledger_packet::Address;
use interledger_service::{Account as AccountTrait, AddressStore};
use interledger_store_redis::{Account, RedisStore, RedisStoreBuilder};
use lazy_static::lazy_static;
use parking_lot::Mutex;
use std::str::FromStr;
use tokio::runtime::Runtime;

lazy_static! {
    static ref TEST_MUTEX: Mutex<()> = Mutex::new(());
}

pub fn test_store() -> impl Future<Item = (RedisStore, TestContext, Vec<Account>), Error = ()> {
    let context = TestContext::new();
    RedisStoreBuilder::new(context.get_client_connection_info(), [0; 32])
        .node_ilp_address(Address::from_str("example.node").unwrap())
        .connect()
        .and_then(|store| {
            let store_clone = store.clone();
            let mut accs = Vec::new();
            store
                .clone()
                .insert_account(ACCOUNT_DETAILS_0.clone())
                .and_then(move |acc| {
                    accs.push(acc.clone());
                    // alice is a Parent, so the store's ilp address is updated to
                    // the value that would be received by the ILDCP request. here,
                    // we just assume alice appended some data to her address
                    store
                        .clone()
                        .set_ilp_address(acc.ilp_address().with_suffix(b"user1").unwrap())
                        .and_then(move |_| {
                            store_clone
                                .insert_account(ACCOUNT_DETAILS_1.clone())
                                .and_then(move |acc| {
                                    accs.push(acc.clone());
                                    Ok((store, context, accs))
                                })
                        })
                })
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
