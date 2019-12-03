#![cfg(feature = "redis")]

use crate::node::InterledgerNode;
use futures::{future::result, Future};
pub use interledger::{
    api::{AccountDetails, NodeStore},
    packet::Address,
    service::Account,
    store::redis::RedisStoreBuilder,
};
pub use redis_crate::{ConnectionInfo, IntoConnectionInfo};
use ring::hmac;
use tracing::{debug, error};
use uuid::Uuid;

static REDIS_SECRET_GENERATION_STRING: &str = "ilp_redis_secret";

pub fn default_redis_url() -> String {
    String::from("redis://127.0.0.1:6379")
}

// This function could theoretically be defined as an inherent method on InterledgerNode itself.
// However, we define it in this module in order to consolidate conditionally-compiled code
// into as few discrete units as possible.
pub fn serve_redis_node(
    node: InterledgerNode,
    ilp_address: Address,
) -> impl Future<Item = (), Error = ()> {
    let redis_connection_info = node.database_url.clone().into_connection_info().unwrap();
    let redis_addr = redis_connection_info.addr.clone();
    let redis_secret = generate_redis_secret(&node.secret_seed);
    Box::new(RedisStoreBuilder::new(redis_connection_info, redis_secret)
    .node_ilp_address(ilp_address.clone())
    .connect()
    .map_err(move |err| error!(target: "interledger-node", "Error connecting to Redis: {:?} {:?}", redis_addr, err))
    .and_then(move |store| node.chain_services(store, ilp_address)))
}

pub fn generate_redis_secret(secret_seed: &[u8; 32]) -> [u8; 32] {
    let mut redis_secret: [u8; 32] = [0; 32];
    let sig = hmac::sign(
        &hmac::Key::new(hmac::HMAC_SHA256, secret_seed),
        REDIS_SECRET_GENERATION_STRING.as_bytes(),
    );
    redis_secret.copy_from_slice(sig.as_ref());
    redis_secret
}

#[doc(hidden)]
#[allow(dead_code)]
#[deprecated(note = "use HTTP API instead")]
pub fn insert_account_with_redis_store(
    node: &InterledgerNode,
    account: AccountDetails,
) -> impl Future<Item = Uuid, Error = ()> {
    let redis_secret = generate_redis_secret(&node.secret_seed);
    result(node.database_url.clone().into_connection_info())
        .map_err(
            |err| error!(target: "interledger-node", "Invalid Redis connection details: {:?}", err),
        )
        .and_then(move |redis_url| RedisStoreBuilder::new(redis_url, redis_secret).connect())
        .map_err(|err| error!(target: "interledger-node", "Error connecting to Redis: {:?}", err))
        .and_then(move |store| {
            store
                .insert_account(account)
                .map_err(|_| error!(target: "interledger-node", "Unable to create account"))
                .and_then(|account| {
                    debug!(target: "interledger-node", "Created account: {}", account.id());
                    Ok(account.id())
                })
        })
}
