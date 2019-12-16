#![cfg(feature = "redis")]

use crate::node::{generate_database_secret, InterledgerNode};
use futures::Future;
use interledger::{packet::Address, store::redis::RedisStoreBuilder};
pub use redis_crate::{ConnectionInfo, IntoConnectionInfo};
use tracing::error;

pub const DEFAULT_REDIS_URL: &str = "redis://127.0.0.1:6379";
const REDIS_SECRET_GENERATION_STRING: &str = "ilp_redis_secret";

// This function could theoretically be defined as an inherent method on InterledgerNode itself.
// However, we define it in this module in order to consolidate conditionally-compiled code
// into as few discrete units as possible.
pub fn serve_redis_node(
    node: InterledgerNode,
    ilp_address: Address,
) -> impl Future<Item = (), Error = ()> {
    let redis_connection_info = node.database_url.clone().into_connection_info().unwrap();
    let redis_addr = redis_connection_info.addr.clone();
    let redis_secret = generate_database_secret(&node.secret_seed, REDIS_SECRET_GENERATION_STRING);
    Box::new(RedisStoreBuilder::new(redis_connection_info, redis_secret)
    .node_ilp_address(ilp_address.clone())
    .connect()
    .map_err(move |err| error!(target: "interledger-node", "Error connecting to Redis: {:?} {:?}", redis_addr, err))
    .and_then(move |store| node.chain_services(store, ilp_address)))
}
