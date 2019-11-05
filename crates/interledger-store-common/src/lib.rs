use futures::Future;
use interledger_api::NodeStore;
use interledger_btp::BtpStore;
use interledger_ccp::RouteManagerStore;
use interledger_http::{idempotency::IdempotentStore, HttpStore};
use interledger_packet::Address;
use interledger_router::RouterStore;
use interledger_service::{AccountStore, AddressStore};
use interledger_service_util::{BalanceStore, ExchangeRateStore, RateLimitStore};
use interledger_settlement::{LeftoversStore, SettlementStore};
use interledger_stream::StreamNotificationsStore;

pub trait StoreBuilder {
    type Store: Clone
        + NodeStore
        + BtpStore
        + RouteManagerStore
        + IdempotentStore
        + HttpStore
        + RouterStore
        + AccountStore
        + AddressStore
        + BalanceStore
        + ExchangeRateStore
        + RateLimitStore
        + LeftoversStore
        + SettlementStore
        + StreamNotificationsStore;

    fn node_ilp_address(&mut self, node_ilp_address: Address) -> &mut Self;

    fn poll_interval(&mut self, poll_interval: u64) -> &mut Self;

    fn connect(&mut self) -> Box<dyn Future<Item = Self::Store, Error = ()> + Send + 'static>;
}
