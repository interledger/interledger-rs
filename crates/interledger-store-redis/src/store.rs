use bytes::Bytes;
use futures::future::Future;
use http::StatusCode;
use interledger_api::NodeStore;
use interledger_api::{AccountDetails, AccountSettings};
use interledger_btp::{BtpAccount, BtpStore};
use interledger_ccp::{CcpRoutingAccount, RouteManagerStore};
use interledger_http::{HttpAccount, HttpStore};
use interledger_packet::Address;
use interledger_router::RouterStore;
use interledger_service::Account as AccountTrait;
use interledger_service::{AccountStore, Username};
use interledger_service_util::{BalanceStore, ExchangeRateStore};
use interledger_service_util::{RateLimitAccount, RateLimitError, RateLimitStore};
use interledger_settlement::{IdempotentData, IdempotentStore};
use interledger_settlement::{SettlementAccount, SettlementStore};
use std::collections::HashMap;
use std::marker::PhantomData;

#[cfg(feature = "redis")]
use crate::backends::redis::{RedisStore, RedisStoreBuilder};
#[cfg(feature = "redis")]
use redis_crate::ConnectionInfo;

type RoutingTable<A> = HashMap<Bytes, A>;

#[derive(Clone)]
pub struct InterledgerStore<S, A> {
    db: S,
    account_type: PhantomData<A>,
    node_ilp_address: Option<Address>,
    username: Username,
}

impl<S, A> InterledgerStore<S, A> {
    pub fn new(db: S, username: Username, node_ilp_address: Option<Address>) -> Self {
        Self {
            db,
            username,
            node_ilp_address,
            account_type: PhantomData,
        }
    }
    // TODO: Add new_inmemory_store(...) & delete ILP Store Memory crate
}

impl<S, A> std::ops::Deref for InterledgerStore<S, A> {
    type Target = S;

    fn deref(&self) -> &Self::Target {
        &self.db
    }
}

#[cfg(feature = "redis")]
impl<A> InterledgerStore<RedisStore, A> {
    pub fn new_redis_store(
        redis_url: ConnectionInfo,
        secret: [u8; 32],
        username: Username,
        node_ilp_address: Option<Address>,
        poll_interval: Option<u64>,
    ) -> impl Future<Item = Self, Error = ()> {
        let mut builder = RedisStoreBuilder::new(redis_url, secret, username.clone());

        if let Some(node_ilp_address) = node_ilp_address.clone() {
            builder.node_ilp_address(node_ilp_address);
        }

        if let Some(poll_interval) = poll_interval {
            builder.poll_interval(poll_interval);
        }

        builder
            .connect()
            .and_then(move |redis_store| Ok(Self::new(redis_store, username, node_ilp_address)))
    }
}

// We forward all calls to the underlying database. A database implementor
// should only write code that involves reading and writing the arguments to the
// database, without having to perform any manipulation to them.
// TODO: Add a cache layer
// TODO: Move PUBSUB functionality here
// TODO: Actually extract the cryptography functionalities in these calls.
impl<S, A> AccountStore for InterledgerStore<S, A>
where
    S: AccountStore<Account = A>,
    A: AccountTrait,
{
    type Account = A;

    fn get_accounts(
        &self,
        account_ids: Vec<<Self::Account as AccountTrait>::AccountId>,
    ) -> Box<dyn Future<Item = Vec<Self::Account>, Error = ()> + Send> {
        self.db.get_accounts(account_ids)
    }

    fn get_account_id_from_username(
        &self,
        username: &Username,
    ) -> Box<dyn Future<Item = <Self::Account as AccountTrait>::AccountId, Error = ()> + Send> {
        self.db.get_account_id_from_username(username)
    }
}

impl<S, A> BalanceStore for InterledgerStore<S, A>
where
    S: BalanceStore<Account = A>,
    A: AccountTrait,
{
    fn get_balance(&self, account: A) -> Box<dyn Future<Item = i64, Error = ()> + Send> {
        self.db.get_balance(account)
    }

    fn update_balances_for_prepare(
        &self,
        from_account: A,
        incoming_amount: u64,
    ) -> Box<dyn Future<Item = (), Error = ()> + Send> {
        self.db
            .update_balances_for_prepare(from_account, incoming_amount)
    }

    fn update_balances_for_fulfill(
        &self,
        to_account: A,
        outgoing_amount: u64,
    ) -> Box<dyn Future<Item = (i64, u64), Error = ()> + Send> {
        self.db
            .update_balances_for_fulfill(to_account, outgoing_amount)
    }

    fn update_balances_for_reject(
        &self,
        from_account: A,
        incoming_amount: u64,
    ) -> Box<dyn Future<Item = (), Error = ()> + Send> {
        self.db
            .update_balances_for_reject(from_account, incoming_amount)
    }
}

impl<S, A> ExchangeRateStore for InterledgerStore<S, A>
where
    S: ExchangeRateStore,
    A: AccountTrait,
{
    fn get_exchange_rates(&self, asset_codes: &[&str]) -> Result<Vec<f64>, ()> {
        self.db.get_exchange_rates(asset_codes)
    }

    fn get_all_exchange_rates(&self) -> Result<HashMap<String, f64>, ()> {
        self.db.get_all_exchange_rates()
    }

    fn set_exchange_rates(&self, rates: HashMap<String, f64>) -> Result<(), ()> {
        self.db.set_exchange_rates(rates)
    }
}

impl<S, A> BtpStore for InterledgerStore<S, A>
where
    S: BtpStore<Account = A>,
    A: BtpAccount,
{
    type Account = A;

    fn get_account_from_btp_auth(
        &self,
        username: &Username,
        token: &str,
    ) -> Box<dyn Future<Item = Self::Account, Error = ()> + Send> {
        self.db.get_account_from_btp_auth(username, token)
    }

    fn get_btp_outgoing_accounts(
        &self,
    ) -> Box<dyn Future<Item = Vec<Self::Account>, Error = ()> + Send> {
        self.db.get_btp_outgoing_accounts()
    }
}

impl<S, A> HttpStore for InterledgerStore<S, A>
where
    S: HttpStore<Account = A>,
    A: HttpAccount + Sync + 'static,
{
    type Account = A;

    fn get_account_from_http_auth(
        &self,
        username: &Username,
        token: &str,
    ) -> Box<dyn Future<Item = Self::Account, Error = ()> + Send> {
        self.db.get_account_from_http_auth(username, token)
    }
}

impl<S, A> RouterStore for InterledgerStore<S, A>
where
    S: RouterStore<Account = A>,
    A: AccountTrait + Sync + 'static,
{
    fn routing_table(&self) -> HashMap<Bytes, <Self::Account as AccountTrait>::AccountId> {
        self.db.routing_table()
    }
}

impl<S, A> NodeStore for InterledgerStore<S, A>
where
    S: NodeStore<Account = A>,
    A: AccountTrait + Sync + 'static,
{
    type Account = A;

    fn insert_account(
        &self,
        account: AccountDetails,
    ) -> Box<dyn Future<Item = Self::Account, Error = ()> + Send> {
        self.db.insert_account(account)
    }

    fn delete_account(
        &self,
        id: <Self::Account as AccountTrait>::AccountId,
    ) -> Box<dyn Future<Item = Self::Account, Error = ()> + Send> {
        self.db.delete_account(id)
    }

    fn update_account(
        &self,
        id: <Self::Account as AccountTrait>::AccountId,
        account: AccountDetails,
    ) -> Box<dyn Future<Item = Self::Account, Error = ()> + Send> {
        self.db.update_account(id, account)
    }

    fn modify_account_settings(
        &self,
        id: <Self::Account as AccountTrait>::AccountId,
        settings: AccountSettings,
    ) -> Box<dyn Future<Item = Self::Account, Error = ()> + Send> {
        self.db.modify_account_settings(id, settings)
    }

    fn get_all_accounts(&self) -> Box<dyn Future<Item = Vec<Self::Account>, Error = ()> + Send> {
        self.db.get_all_accounts()
    }

    fn set_static_routes<R>(&self, routes: R) -> Box<dyn Future<Item = (), Error = ()> + Send>
    where
        R: IntoIterator<Item = (String, <Self::Account as AccountTrait>::AccountId)>,
    {
        self.db.set_static_routes(routes)
    }

    fn set_static_route(
        &self,
        prefix: String,
        account_id: <Self::Account as AccountTrait>::AccountId,
    ) -> Box<dyn Future<Item = (), Error = ()> + Send> {
        self.db.set_static_route(prefix, account_id)
    }

    fn set_ilp_address(&self, ilp_address: Address) {
        self.db.set_ilp_address(ilp_address)
    }
}

impl<S, A> RouteManagerStore for InterledgerStore<S, A>
where
    S: RouteManagerStore<Account = A>,
    A: CcpRoutingAccount + Sync + 'static,
{
    type Account = A;

    fn get_accounts_to_send_routes_to(
        &self,
    ) -> Box<dyn Future<Item = Vec<Self::Account>, Error = ()> + Send> {
        self.db.get_accounts_to_send_routes_to()
    }

    fn get_accounts_to_receive_routes_from(
        &self,
    ) -> Box<dyn Future<Item = Vec<Self::Account>, Error = ()> + Send> {
        self.db.get_accounts_to_receive_routes_from()
    }

    fn get_local_and_configured_routes(
        &self,
    ) -> Box<
        dyn Future<Item = (RoutingTable<Self::Account>, RoutingTable<Self::Account>), Error = ()>
            + Send,
    > {
        self.db.get_local_and_configured_routes()
    }

    fn set_routes(
        &mut self,
        routes: impl IntoIterator<Item = (Bytes, Self::Account)>,
    ) -> Box<dyn Future<Item = (), Error = ()> + Send> {
        self.db.set_routes(routes)
    }
}

impl<S, A> RateLimitStore for InterledgerStore<S, A>
where
    S: RateLimitStore<Account = A>,
    A: RateLimitAccount,
{
    type Account = A;

    fn apply_rate_limits(
        &self,
        account: Self::Account,
        prepare_amount: u64,
    ) -> Box<dyn Future<Item = (), Error = RateLimitError> + Send> {
        self.db.apply_rate_limits(account, prepare_amount)
    }

    fn refund_throughput_limit(
        &self,
        account: Self::Account,
        prepare_amount: u64,
    ) -> Box<dyn Future<Item = (), Error = ()> + Send> {
        self.db.refund_throughput_limit(account, prepare_amount)
    }
}

impl<S, A> IdempotentStore for InterledgerStore<S, A>
where
    S: IdempotentStore,
{
    fn load_idempotent_data(
        &self,
        idempotency_key: String,
    ) -> Box<dyn Future<Item = Option<IdempotentData>, Error = ()> + Send> {
        self.db.load_idempotent_data(idempotency_key)
    }

    fn save_idempotent_data(
        &self,
        idempotency_key: String,
        input_hash: [u8; 32],
        status_code: StatusCode,
        data: Bytes,
    ) -> Box<dyn Future<Item = (), Error = ()> + Send> {
        self.db
            .save_idempotent_data(idempotency_key, input_hash, status_code, data)
    }
}

impl<S, A> SettlementStore for InterledgerStore<S, A>
where
    S: SettlementStore<Account = A>,
    A: SettlementAccount,
{
    type Account = A;

    fn update_balance_for_incoming_settlement(
        &self,
        account_id: <Self::Account as AccountTrait>::AccountId,
        amount: u64,
        idempotency_key: Option<String>,
    ) -> Box<dyn Future<Item = (), Error = ()> + Send> {
        self.db
            .update_balance_for_incoming_settlement(account_id, amount, idempotency_key)
    }

    fn refund_settlement(
        &self,
        account_id: <Self::Account as AccountTrait>::AccountId,
        settle_amount: u64,
    ) -> Box<dyn Future<Item = (), Error = ()> + Send> {
        self.db.refund_settlement(account_id, settle_amount)
    }
}
