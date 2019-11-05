use std::{collections::HashMap, sync::Arc};

use interledger_api::{AccountDetails, AccountSettings, NodeStore};
use interledger_btp::BtpStore;
use interledger_ccp::RouteManagerStore;
use interledger_http::{
    idempotency::{IdempotentData, IdempotentStore},
    HttpStore,
};
use interledger_packet::Address;
use interledger_router::RouterStore;
use interledger_service::{Account, AccountId, AccountStore, AddressStore, Username};
use interledger_service_util::{BalanceStore, ExchangeRateStore, RateLimitError, RateLimitStore};
use interledger_settlement::{LeftoversStore, SettlementStore};
use interledger_store_common::StoreBuilder;
use interledger_stream::{PaymentNotification, StreamNotificationsStore};

use bytes::Bytes;
use futures::{sync::mpsc::UnboundedSender, Future};
use http::StatusCode;
use num_bigint::BigUint;
use url::Url;

pub struct SqliteStoreBuilder;

impl SqliteStoreBuilder {
    pub fn new() -> Self {
        SqliteStoreBuilder
    }
}

impl StoreBuilder for SqliteStoreBuilder {
    type Store = SqliteStore;

    fn node_ilp_address(&mut self, node_ilp_address: Address) -> &mut Self {
        unimplemented!()
    }

    fn poll_interval(&mut self, poll_interval: u64) -> &mut Self {
        unimplemented!()
    }

    fn connect(&mut self) -> Box<dyn Future<Item = Self::Store, Error = ()> + Send + 'static> {
        unimplemented!()
    }
}

#[derive(Clone)]
pub struct SqliteStore;

impl NodeStore for SqliteStore {
    fn insert_account(
        &self,
        account: AccountDetails,
    ) -> Box<dyn Future<Item = Account, Error = ()> + Send> {
        unimplemented!()
    }

    fn delete_account(&self, id: AccountId) -> Box<dyn Future<Item = Account, Error = ()> + Send> {
        unimplemented!()
    }

    fn update_account(
        &self,
        id: AccountId,
        account: AccountDetails,
    ) -> Box<dyn Future<Item = Account, Error = ()> + Send> {
        unimplemented!()
    }

    fn modify_account_settings(
        &self,
        id: AccountId,
        settings: AccountSettings,
    ) -> Box<dyn Future<Item = Account, Error = ()> + Send> {
        unimplemented!()
    }

    fn get_all_accounts(&self) -> Box<dyn Future<Item = Vec<Account>, Error = ()> + Send> {
        unimplemented!()
    }

    fn set_static_routes<R>(&self, routes: R) -> Box<dyn Future<Item = (), Error = ()> + Send>
    where
        R: IntoIterator<Item = (String, AccountId)>,
    {
        unimplemented!()
    }

    fn set_static_route(
        &self,
        prefix: String,
        account_id: AccountId,
    ) -> Box<dyn Future<Item = (), Error = ()> + Send> {
        unimplemented!()
    }

    fn set_default_route(
        &self,
        account_id: AccountId,
    ) -> Box<dyn Future<Item = (), Error = ()> + Send> {
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

impl BtpStore for SqliteStore {
    fn get_account_from_btp_auth(
        &self,
        username: &Username,
        token: &str,
    ) -> Box<dyn Future<Item = Account, Error = ()> + Send> {
        unimplemented!()
    }

    fn get_btp_outgoing_accounts(&self) -> Box<dyn Future<Item = Vec<Account>, Error = ()> + Send> {
        unimplemented!()
    }
}

type RoutingTable<A> = HashMap<String, A>;

impl RouteManagerStore for SqliteStore {
    fn get_accounts_to_send_routes_to(
        &self,
        ignore_accounts: Vec<AccountId>,
    ) -> Box<dyn Future<Item = Vec<Account>, Error = ()> + Send> {
        unimplemented!()
    }

    fn get_accounts_to_receive_routes_from(
        &self,
    ) -> Box<dyn Future<Item = Vec<Account>, Error = ()> + Send> {
        unimplemented!()
    }

    fn get_local_and_configured_routes(
        &self,
    ) -> Box<dyn Future<Item = (RoutingTable<Account>, RoutingTable<Account>), Error = ()> + Send>
    {
        unimplemented!()
    }

    fn set_routes(
        &mut self,
        routes: impl IntoIterator<Item = (String, Account)>,
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

impl HttpStore for SqliteStore {
    fn get_account_from_http_auth(
        &self,
        username: &Username,
        token: &str,
    ) -> Box<dyn Future<Item = Account, Error = ()> + Send> {
        unimplemented!()
    }
}

impl RouterStore for SqliteStore {
    fn routing_table(&self) -> Arc<HashMap<String, AccountId>> {
        unimplemented!()
    }
}

impl AccountStore for SqliteStore {
    fn get_accounts(
        &self,
        account_ids: Vec<AccountId>,
    ) -> Box<dyn Future<Item = Vec<Account>, Error = ()> + Send> {
        unimplemented!()
    }

    fn get_account_id_from_username(
        &self,
        username: &Username,
    ) -> Box<dyn Future<Item = AccountId, Error = ()> + Send> {
        unimplemented!()
    }
}

impl AddressStore for SqliteStore {
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
        unimplemented!()
    }
}

impl BalanceStore for SqliteStore {
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

impl RateLimitStore for SqliteStore {
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

impl LeftoversStore for SqliteStore {
    fn get_uncredited_settlement_amount(
        &self,
        account_id: AccountId,
    ) -> Box<dyn Future<Item = (BigUint, u8), Error = ()> + Send> {
        unimplemented!()
    }

    fn save_uncredited_settlement_amount(
        &self,
        account_id: AccountId,
        uncredited_settlement_amount: (BigUint, u8),
    ) -> Box<dyn Future<Item = (), Error = ()> + Send> {
        unimplemented!()
    }

    fn load_uncredited_settlement_amount(
        &self,
        account_id: AccountId,
        local_scale: u8,
    ) -> Box<dyn Future<Item = BigUint, Error = ()> + Send> {
        unimplemented!()
    }
}

impl SettlementStore for SqliteStore {
    fn update_balance_for_incoming_settlement(
        &self,
        account_id: AccountId,
        amount: u64,
        idempotency_key: Option<String>,
    ) -> Box<dyn Future<Item = (), Error = ()> + Send> {
        unimplemented!()
    }

    fn refund_settlement(
        &self,
        account_id: AccountId,
        settle_amount: u64,
    ) -> Box<dyn Future<Item = (), Error = ()> + Send> {
        unimplemented!()
    }
}

impl StreamNotificationsStore for SqliteStore {
    fn add_payment_notification_subscription(
        &self,
        id: AccountId,
        sender: UnboundedSender<PaymentNotification>,
    ) {
        unimplemented!()
    }

    fn publish_payment_notification(&self, payment: PaymentNotification) {
        unimplemented!()
    }
}
