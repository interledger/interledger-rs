#![cfg(feature = "sqlite")]

use crate::node::{generate_database_secret, InterledgerNode};
use futures::Future;
use interledger::{packet::Address, store::sqlite::SqliteStoreBuilder};
use tracing::error;

pub const DEFAULT_SQLITE_URL: &str = "sqlite:ilp-node.sqlite3";
const SQLITE_SECRET_GENERATION_STRING: &str = "ilp_sqlite_secret";

// This function could theoretically be defined as an inherent method on InterledgerNode itself.
// However, we define it in this module in order to consolidate conditionally-compiled code
// into as few discrete units as possible.
pub fn serve_sqlite_node(
    node: InterledgerNode,
    ilp_address: Address,
) -> impl Future<Item = (), Error = ()> {
    let sqlite_addr = node.database_url.clone();
    let sqlite_secret =
        generate_database_secret(&node.secret_seed, SQLITE_SECRET_GENERATION_STRING);
    Box::new(SqliteStoreBuilder::new(node.database_url.clone(), sqlite_secret)
    .node_ilp_address(ilp_address.clone())
    .connect()
    .map_err(move |err| error!(target: "interledger-node", "Error connecting to SQLite: {:?} {:?}", sqlite_addr, err))
    .and_then(move |store| node.chain_services(store, ilp_address)))
}
