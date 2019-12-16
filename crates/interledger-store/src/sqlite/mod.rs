#![allow(unused_imports)]
#![allow(unused_variables)]
//! Data store for Interledger.rs using SQLite

use super::account::{Account, AccountWithEncryptedTokens};
use super::crypto::{encrypt_token, generate_keys, DecryptionKey, EncryptionKey};
use bytes::Bytes;
use futures::{
    future::{err, ok, result, Either},
    sync::mpsc::UnboundedSender,
    Future, Stream,
};
use http::StatusCode;
use interledger_api::{AccountDetails, AccountSettings, EncryptedAccountSettings, NodeStore};
use interledger_btp::BtpStore;
use interledger_ccp::{CcpRoutingAccount, RouteManagerStore, RoutingRelation};
use interledger_http::HttpStore;
use interledger_packet::Address;
use interledger_router::RouterStore;
use interledger_service::{Account as AccountTrait, AccountStore, AddressStore, Username};
use interledger_service_util::{
    BalanceStore, ExchangeRateStore, RateLimitError, RateLimitStore, DEFAULT_ROUND_TRIP_TIME,
};
use interledger_settlement::core::{
    idempotency::{IdempotentData, IdempotentStore},
    scale_with_precision_loss,
    types::{Convert, ConvertDetails, LeftoversStore, SettlementStore},
};
use interledger_stream::{PaymentNotification, StreamNotificationsStore};
use lazy_static::lazy_static;
use log::{debug, error, trace, warn};
use num_bigint::BigUint;
use parking_lot::RwLock;
use rusqlite::Connection;
use secrecy::{ExposeSecret, Secret, SecretBytes};
use serde::{Deserialize, Serialize};
use serde_json;
use std::{
    collections::{HashMap, HashSet},
    fmt::Display,
};
use std::{
    iter::{self, FromIterator},
    str,
    str::FromStr,
    sync::Arc,
    time::{Duration, Instant},
};
use tokio_executor::spawn;
use tokio_timer::Interval;
use url::Url;
use uuid::Uuid;
use zeroize::Zeroize;

/*const ACCOUNT_DETAILS_FIELDS: usize = 21;

static PARENT_ILP_KEY: &str = "parent_node_account_address";
static ROUTES_KEY: &str = "routes:current";
static STATIC_ROUTES_KEY: &str = "routes:static";
static DEFAULT_ROUTE_KEY: &str = "routes:default";
static STREAM_NOTIFICATIONS_PREFIX: &str = "stream_notifications:";
static SETTLEMENT_ENGINES_KEY: &str = "settlement_engines";

fn uncredited_amount_key(account_id: impl ToString) -> String {
    format!("uncredited-amount:{}", account_id.to_string())
}

fn prefixed_idempotency_key(idempotency_key: String) -> String {
    format!("idempotency-key:{}", idempotency_key)
}

fn accounts_key(account_id: Uuid) -> String {
    format!("accounts:{}", account_id)
}*/

lazy_static! {
    static ref DEFAULT_ILP_ADDRESS: Address = Address::from_str("local.host").unwrap();
}

pub struct SqliteStoreBuilder {
    sqlite_url: Url,
    secret: [u8; 32],
    //poll_interval: u64, // TODO: if this store is for single-node use cases, do we need to poll at all?
    node_ilp_address: Address,
}

impl SqliteStoreBuilder {
    pub fn new(sqlite_url: Url, secret: [u8; 32]) -> Self {
        SqliteStoreBuilder {
            sqlite_url,
            secret,
            node_ilp_address: DEFAULT_ILP_ADDRESS.clone(),
        }
    }

    pub fn node_ilp_address(&mut self, node_ilp_address: Address) -> &mut Self {
        self.node_ilp_address = node_ilp_address;
        self
    }

    pub fn connect(&mut self) -> impl Future<Item = SqliteStore, Error = ()> {
        let (encryption_key, decryption_key) = generate_keys(&self.secret[..]);
        self.secret.zeroize(); // clear the secret after it has been used for key generation
        let ilp_address = self.node_ilp_address.clone();
        let sqlite_path = self.sqlite_url.path();
        let store = SqliteStore {
            ilp_address: Arc::new(RwLock::new(ilp_address)),
            connection: SqliteConnection {
                path: format!("file:{}?mode=memory&cache=shared", sqlite_path),
            },
            subscriptions: Arc::new(RwLock::new(HashMap::new())),
            exchange_rates: Arc::new(RwLock::new(HashMap::new())),
            routes: Arc::new(RwLock::new(Arc::new(HashMap::new()))),
            encryption_key: Arc::new(encryption_key),
            decryption_key: Arc::new(decryption_key),
        };

        // TODO: use parent address if found
        // TODO: poll for routing table updates
        // TODO: pub/sub listener for outgoing notifications over websockets
        ok(store)
    }
}

#[derive(Clone)]
struct SqliteConnection {
    path: String,
}

/*
impl SqliteConnection {
    // TODO: using a connection pool would prevent us from needing to create a new connection every time,
    // but creating a cached connection to an in-memory store might be fast enough in practice?
    fn get(&self) -> rusqlite::Result<Connection> {
        Connection::open(&self.path)
    }
}
*/

/// A Store that uses SQLite as its underlying database.
///
/// This store leverages atomic SQLite transactions to do operations such as balance updates.
///
/// Currently the SQLiteStore polls the database for the routing table and rate updates, but
/// future versions of it will use PubSub to subscribe to updates.
#[derive(Clone)]
pub struct SqliteStore {
    ilp_address: Arc<RwLock<Address>>,
    // TODO: use an async connection pool, none of which yet support rusqlite
    connection: SqliteConnection,
    subscriptions: Arc<RwLock<HashMap<Uuid, UnboundedSender<PaymentNotification>>>>,
    exchange_rates: Arc<RwLock<HashMap<String, f64>>>,
    /// The store keeps the routing table in memory so that it can be returned
    /// synchronously while the Router is processing packets.
    /// The outer `Arc<RwLock>` is used so that we can update the stored routing
    /// table after polling the store for updates.
    /// The inner `Arc<HashMap>` is used so that the `routing_table` method can
    /// return a reference to the routing table without cloning the underlying data.
    routes: Arc<RwLock<Arc<HashMap<String, Uuid>>>>,
    encryption_key: Arc<Secret<EncryptionKey>>,
    decryption_key: Arc<Secret<DecryptionKey>>,
}

impl SqliteStore {
    // TODO: all of this
}

impl AccountStore for SqliteStore {
    type Account = Account;

    fn get_accounts(
        &self,
        account_ids: Vec<Uuid>,
    ) -> Box<dyn Future<Item = Vec<Account>, Error = ()> + Send> {
        unimplemented!()
    }

    fn get_account_id_from_username(
        &self,
        username: &Username,
    ) -> Box<dyn Future<Item = Uuid, Error = ()> + Send> {
        unimplemented!()
    }
}

impl StreamNotificationsStore for SqliteStore {
    type Account = Account;

    fn add_payment_notification_subscription(
        &self,
        id: Uuid,
        sender: UnboundedSender<PaymentNotification>,
    ) {
        unimplemented!()
    }

    fn publish_payment_notification(&self, payment: PaymentNotification) {
        unimplemented!()
    }
}

impl BalanceStore for SqliteStore {
    /// Returns the balance **from the account holder's perspective**, meaning the sum of
    /// the Payable Balance and Pending Outgoing minus the Receivable Balance and the Pending Incoming.
    fn get_balance(&self, account: Account) -> Box<dyn Future<Item = i64, Error = ()> + Send> {
        unimplemented!()
    }

    fn update_balances_for_prepare(
        &self,
        from_account: Account, // TODO: Make this take only the id
        incoming_amount: u64,
    ) -> Box<dyn Future<Item = (), Error = ()> + Send> {
        unimplemented!()
    }

    fn update_balances_for_fulfill(
        &self,
        to_account: Account, // TODO: Make this take only the id
        outgoing_amount: u64,
    ) -> Box<dyn Future<Item = (i64, u64), Error = ()> + Send> {
        unimplemented!()
    }

    fn update_balances_for_reject(
        &self,
        from_account: Account, // TODO: Make this take only the id
        incoming_amount: u64,
    ) -> Box<dyn Future<Item = (), Error = ()> + Send> {
        unimplemented!()
    }
}

impl ExchangeRateStore for SqliteStore {
    fn get_exchange_rates(&self, asset_codes: &[&str]) -> Result<Vec<f64>, ()> {
        unimplemented!()
    }

    fn get_all_exchange_rates(&self) -> Result<HashMap<String, f64>, ()> {
        unimplemented!()
    }

    fn set_exchange_rates(&self, rates: HashMap<String, f64>) -> Result<(), ()> {
        unimplemented!()
    }
}

impl BtpStore for SqliteStore {
    type Account = Account;

    fn get_account_from_btp_auth(
        &self,
        username: &Username,
        token: &str,
    ) -> Box<dyn Future<Item = Self::Account, Error = ()> + Send> {
        unimplemented!()
    }

    fn get_btp_outgoing_accounts(
        &self,
    ) -> Box<dyn Future<Item = Vec<Self::Account>, Error = ()> + Send> {
        // TODO: non-stub implementation
        Box::new(ok(Vec::new()))
    }
}

impl HttpStore for SqliteStore {
    type Account = Account;

    /// Checks if the stored token for the provided account id matches the
    /// provided token, and if so, returns the account associated with that token
    fn get_account_from_http_auth(
        &self,
        username: &Username,
        token: &str,
    ) -> Box<dyn Future<Item = Self::Account, Error = ()> + Send> {
        unimplemented!()
    }
}

impl RouterStore for SqliteStore {
    fn routing_table(&self) -> Arc<HashMap<String, Uuid>> {
        unimplemented!()
    }
}

impl NodeStore for SqliteStore {
    type Account = Account;

    fn insert_account(
        &self,
        account: AccountDetails,
    ) -> Box<dyn Future<Item = Account, Error = ()> + Send> {
        unimplemented!()
    }

    fn delete_account(&self, id: Uuid) -> Box<dyn Future<Item = Account, Error = ()> + Send> {
        unimplemented!()
    }

    fn update_account(
        &self,
        id: Uuid,
        account: AccountDetails,
    ) -> Box<dyn Future<Item = Self::Account, Error = ()> + Send> {
        unimplemented!()
    }

    fn modify_account_settings(
        &self,
        id: Uuid,
        settings: AccountSettings,
    ) -> Box<dyn Future<Item = Self::Account, Error = ()> + Send> {
        unimplemented!()
    }

    // TODO limit the number of results and page through them
    fn get_all_accounts(&self) -> Box<dyn Future<Item = Vec<Self::Account>, Error = ()> + Send> {
        unimplemented!()
    }

    fn set_static_routes<R>(&self, routes: R) -> Box<dyn Future<Item = (), Error = ()> + Send>
    where
        R: IntoIterator<Item = (String, Uuid)>,
    {
        unimplemented!()
    }

    fn set_static_route(
        &self,
        prefix: String,
        account_id: Uuid,
    ) -> Box<dyn Future<Item = (), Error = ()> + Send> {
        unimplemented!()
    }

    fn set_default_route(&self, account_id: Uuid) -> Box<dyn Future<Item = (), Error = ()> + Send> {
        unimplemented!()
    }

    fn set_settlement_engines(
        &self,
        asset_to_url_map: impl IntoIterator<Item = (String, Url)>,
    ) -> Box<dyn Future<Item = (), Error = ()> + Send> {
        unimplemented!()
    }

    fn get_asset_settlement_engine(
        &self,
        asset_code: &str,
    ) -> Box<dyn Future<Item = Option<Url>, Error = ()> + Send> {
        unimplemented!()
    }
}

impl AddressStore for SqliteStore {
    // Updates the ILP address of the store & iterates over all children and
    // updates their ILP Address to match the new address.
    fn set_ilp_address(
        &self,
        ilp_address: Address,
    ) -> Box<dyn Future<Item = (), Error = ()> + Send> {
        unimplemented!()
    }

    fn clear_ilp_address(&self) -> Box<dyn Future<Item = (), Error = ()> + Send> {
        unimplemented!()
    }

    fn get_ilp_address(&self) -> Address {
        // read consumes the Arc<RwLock<T>> so we cannot return a reference
        self.ilp_address.read().clone()
    }
}

type RoutingTable<A> = HashMap<String, A>;

impl RouteManagerStore for SqliteStore {
    type Account = Account;

    fn get_accounts_to_send_routes_to(
        &self,
        ignore_accounts: Vec<Uuid>,
    ) -> Box<dyn Future<Item = Vec<Account>, Error = ()> + Send> {
        // TODO: non-stub implementation
        Box::new(ok(Vec::new()))
    }

    fn get_accounts_to_receive_routes_from(
        &self,
    ) -> Box<dyn Future<Item = Vec<Account>, Error = ()> + Send> {
        // TODO: non-stub implementation
        Box::new(ok(Vec::new()))
    }

    fn get_local_and_configured_routes(
        &self,
    ) -> Box<dyn Future<Item = (RoutingTable<Account>, RoutingTable<Account>), Error = ()> + Send>
    {
        // TODO: non-stub implementation
        Box::new(ok((HashMap::new(), HashMap::new())))
    }

    fn set_routes(
        &mut self,
        routes: impl IntoIterator<Item = (String, Account)>,
    ) -> Box<dyn Future<Item = (), Error = ()> + Send> {
        unimplemented!()
    }
}

impl RateLimitStore for SqliteStore {
    type Account = Account;

    /// Apply rate limits for number of packets per minute and amount of money per minute
    fn apply_rate_limits(
        &self,
        account: Account,
        prepare_amount: u64,
    ) -> Box<dyn Future<Item = (), Error = RateLimitError> + Send> {
        unimplemented!()
    }

    fn refund_throughput_limit(
        &self,
        account: Account,
        prepare_amount: u64,
    ) -> Box<dyn Future<Item = (), Error = ()> + Send> {
        unimplemented!()
    }
}

impl IdempotentStore for SqliteStore {
    fn load_idempotent_data(
        &self,
        idempotency_key: String,
    ) -> Box<dyn Future<Item = Option<IdempotentData>, Error = ()> + Send> {
        unimplemented!()
    }

    fn save_idempotent_data(
        &self,
        idempotency_key: String,
        input_hash: [u8; 32],
        status_code: StatusCode,
        data: Bytes,
    ) -> Box<dyn Future<Item = (), Error = ()> + Send> {
        unimplemented!()
    }
}

impl SettlementStore for SqliteStore {
    type Account = Account;

    fn update_balance_for_incoming_settlement(
        &self,
        account_id: Uuid,
        amount: u64,
        idempotency_key: Option<String>,
    ) -> Box<dyn Future<Item = (), Error = ()> + Send> {
        unimplemented!()
    }

    fn refund_settlement(
        &self,
        account_id: Uuid,
        settle_amount: u64,
    ) -> Box<dyn Future<Item = (), Error = ()> + Send> {
        unimplemented!()
    }
}

impl LeftoversStore for SqliteStore {
    type AccountId = Uuid;
    type AssetType = BigUint;

    fn get_uncredited_settlement_amount(
        &self,
        account_id: Self::AccountId,
    ) -> Box<dyn Future<Item = (Self::AssetType, u8), Error = ()> + Send> {
        unimplemented!()
    }

    fn save_uncredited_settlement_amount(
        &self,
        account_id: Self::AccountId,
        uncredited_settlement_amount: (Self::AssetType, u8),
    ) -> Box<dyn Future<Item = (), Error = ()> + Send> {
        unimplemented!()
    }

    fn load_uncredited_settlement_amount(
        &self,
        account_id: Self::AccountId,
        local_scale: u8,
    ) -> Box<dyn Future<Item = Self::AssetType, Error = ()> + Send> {
        unimplemented!()
    }

    fn clear_uncredited_settlement_amount(
        &self,
        account_id: Self::AccountId,
    ) -> Box<dyn Future<Item = (), Error = ()> + Send> {
        unimplemented!()
    }
}
