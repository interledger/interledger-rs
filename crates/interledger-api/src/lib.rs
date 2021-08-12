#![type_length_limit = "1707074"]
use async_trait::async_trait;
use bytes::Bytes;
use interledger_btp::{BtpAccount, BtpOutgoingService};
use interledger_ccp::CcpRoutingAccount;
use interledger_errors::NodeStoreError;
use interledger_http::{HttpAccount, HttpStore};
use interledger_packet::Address;
use interledger_rates::ExchangeRateStore;
use interledger_router::RouterStore;
use interledger_service::{
    Account, AccountStore, AddressStore, IncomingService, OutgoingService, Username,
};
use interledger_service_util::BalanceStore;
use interledger_settlement::core::types::{SettlementAccount, SettlementStore};
use interledger_stream::StreamNotificationsStore;
use secrecy::SecretString;
use serde::{de, Deserialize, Serialize};
use std::{boxed::*, collections::HashMap, fmt::Display, net::SocketAddr, str::FromStr};
use url::Url;
use uuid::Uuid;
use warp::{self, Filter};

mod routes;

// This enum and the following functions are used to allow clients to send either
// numbers or strings and have them be properly deserialized into the appropriate
// integer type.
#[derive(Deserialize)]
#[serde(untagged)]
enum NumOrStr<T> {
    Num(T),
    Str(String),
}

pub fn number_or_string<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    D: de::Deserializer<'de>,
    T: FromStr + Deserialize<'de>,
    <T as FromStr>::Err: Display,
{
    match NumOrStr::deserialize(deserializer)? {
        NumOrStr::Num(n) => Ok(n),
        NumOrStr::Str(s) => T::from_str(&s).map_err(de::Error::custom),
    }
}

pub fn optional_number_or_string<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: de::Deserializer<'de>,
    T: FromStr + Deserialize<'de>,
    <T as FromStr>::Err: Display,
{
    match NumOrStr::deserialize(deserializer)? {
        NumOrStr::Num(n) => Ok(Some(n)),
        NumOrStr::Str(s) => T::from_str(&s).map_err(de::Error::custom).map(Some),
    }
}

pub fn map_of_number_or_string<'de, D>(deserializer: D) -> Result<HashMap<String, f64>, D::Error>
where
    D: de::Deserializer<'de>,
{
    #[derive(Deserialize)]
    struct Wrapper(#[serde(deserialize_with = "number_or_string")] f64);

    let v = HashMap::<String, Wrapper>::deserialize(deserializer)?;
    Ok(v.into_iter().map(|(k, Wrapper(v))| (k, v)).collect())
}

fn serialize_optional_secret_string<S>(
    secret: &Option<SecretString>,
    serializer: S,
) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    use secrecy::ExposeSecret;
    match secret {
        Some(secret) => secret.expose_secret().serialize(serializer),
        None => Option::<String>::None.serialize(serializer),
    }
}

// TODO should the methods from this trait be split up and put into the
// traits that are more specific to what they're doing?
// One argument against doing that is that the NodeStore allows admin-only
// modifications to the values, whereas many of the other traits mostly
// read from the configured values.
#[async_trait]
pub trait NodeStore: Clone + Send + Sync + 'static {
    type Account: Account;

    /// Inserts an account to the store. Generates a UUID and returns the full Account object.
    async fn insert_account(
        &self,
        account: AccountDetails,
    ) -> Result<Self::Account, NodeStoreError>;

    /// Deletes the account corresponding to the provided id and returns it
    async fn delete_account(&self, id: Uuid) -> Result<Self::Account, NodeStoreError>;

    /// Overwrites the account corresponding to the provided id with the provided details
    async fn update_account(
        &self,
        id: Uuid,
        account: AccountDetails,
    ) -> Result<Self::Account, NodeStoreError>;

    /// Modifies the account corresponding to the provided id with the provided settings.
    /// `modify_account_settings` allows **users** to update their account settings with a set of
    /// limited fields of account details. However `update_account` allows **admins** to fully
    /// update account settings.
    async fn modify_account_settings(
        &self,
        id: Uuid,
        settings: AccountSettings,
    ) -> Result<Self::Account, NodeStoreError>;

    // TODO limit the number of results and page through them
    /// Gets all stored accounts
    async fn get_all_accounts(&self) -> Result<Vec<Self::Account>, NodeStoreError>;

    /// Sets the static routes for routing
    async fn set_static_routes<R>(&self, routes: R) -> Result<(), NodeStoreError>
    where
        // The 'async_trait lifetime is used after recommendation here:
        // https://github.com/dtolnay/async-trait/issues/8#issuecomment-514812245
        R: IntoIterator<Item = (String, Uuid)> + Send + 'async_trait;

    /// Sets a single static route
    async fn set_static_route(
        &self,
        prefix: String,
        account_id: Uuid,
    ) -> Result<(), NodeStoreError>;

    /// Sets the default route ("") to be the provided account id
    /// (acts as a catch-all route if all other routes don't match)
    async fn set_default_route(&self, account_id: Uuid) -> Result<(), NodeStoreError>;

    /// Sets the default settlement engines to be used for the provided asset codes
    async fn set_settlement_engines(
        &self,
        // The 'async_trait lifetime is used after recommendation here:
        // https://github.com/dtolnay/async-trait/issues/8#issuecomment-514812245
        asset_to_url_map: impl IntoIterator<Item = (String, Url)> + Send + 'async_trait,
    ) -> Result<(), NodeStoreError>;

    /// Gets the default settlement engine for the provided asset code
    async fn get_asset_settlement_engine(
        &self,
        asset_code: &str,
    ) -> Result<Option<Url>, NodeStoreError>;
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExchangeRates(
    #[serde(deserialize_with = "map_of_number_or_string")] HashMap<String, f64>,
);

/// AccountSettings is a subset of the user parameters defined in
/// AccountDetails. Its purpose is to allow a user to modify certain of their
/// parameters which they may want to re-configure in the future, such as their
/// tokens (which act as passwords), their settlement frequency preferences, or
/// their HTTP/BTP endpoints, since they may change their network configuration.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AccountSettings {
    /// The account's incoming ILP over HTTP token.
    #[serde(default, serialize_with = "serialize_optional_secret_string")]
    pub ilp_over_http_incoming_token: Option<SecretString>,
    /// The account's incoming ILP over BTP token.
    #[serde(default, serialize_with = "serialize_optional_secret_string")]
    pub ilp_over_btp_incoming_token: Option<SecretString>,
    /// The account's outgoing ILP over HTTP token
    #[serde(default, serialize_with = "serialize_optional_secret_string")]
    pub ilp_over_http_outgoing_token: Option<SecretString>,
    /// The account's outgoing ILP over BTP token.
    /// This must match the ILP over BTP incoming token on the peer's node if exchanging
    /// packets with that peer.
    #[serde(default, serialize_with = "serialize_optional_secret_string")]
    pub ilp_over_btp_outgoing_token: Option<SecretString>,
    /// The account's ILP over HTTP URL (this is where packets are sent over HTTP from your node)
    pub ilp_over_http_url: Option<String>,
    /// The account's ILP over BTP URL (this is where packets are sent over WebSockets from your node)
    pub ilp_over_btp_url: Option<String>,
    /// The threshold after which the balance service will trigger a settlement
    #[serde(default, deserialize_with = "optional_number_or_string")]
    pub settle_threshold: Option<i64>,
    /// The amount which the balance service will attempt to settle down to.
    /// Note that this is intentionally an unsigned integer because users should
    /// not be able to set the settle_to value to be negative (meaning the node
    /// would pre-fund with the user)
    #[serde(default, deserialize_with = "optional_number_or_string")]
    pub settle_to: Option<u64>,
}

/// EncryptedAccountSettings is created by encrypting the incoming and outgoing
/// HTTP and BTP tokens of an AccountSettings object. The rest of the fields
/// remain the same. It is intended to be consumed by the internal store
/// implementation which operates only on encrypted data.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct EncryptedAccountSettings {
    pub ilp_over_http_incoming_token: Option<Bytes>,
    pub ilp_over_btp_incoming_token: Option<Bytes>,
    pub ilp_over_http_outgoing_token: Option<Bytes>,
    pub ilp_over_btp_outgoing_token: Option<Bytes>,
    pub ilp_over_http_url: Option<String>,
    pub ilp_over_btp_url: Option<String>,
    #[serde(default, deserialize_with = "optional_number_or_string")]
    /// The threshold after which the balance service will trigger a settlement
    pub settle_threshold: Option<i64>,
    #[serde(default, deserialize_with = "optional_number_or_string")]
    /// The amount which the balance service will attempt to settle down to
    pub settle_to: Option<u64>,
}

/// The Account type for the RedisStore.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AccountDetails {
    /// The account's Interledger Protocol address.
    /// If none is provided, the node should generate one
    pub ilp_address: Option<Address>,
    /// The account's username
    pub username: Username,
    /// The account's currency
    pub asset_code: String,
    #[serde(deserialize_with = "number_or_string")]
    /// The account's asset scale
    pub asset_scale: u8,
    #[serde(default = "u64::max_value", deserialize_with = "number_or_string")]
    /// The max amount per packet which can be routed for this account
    pub max_packet_amount: u64,
    /// The minimum balance this account can have (consider this as a credit/trust limit)
    #[serde(default, deserialize_with = "optional_number_or_string")]
    pub min_balance: Option<i64>,
    /// The account's ILP over HTTP URL (this is where packets are sent over HTTP from your node)
    pub ilp_over_http_url: Option<String>,
    /// The account's API and incoming ILP over HTTP token.
    /// This must match the ILP over HTTP outgoing token on the peer's node if receiving
    /// packets from that peer
    // TODO: The incoming token is used for both ILP over HTTP, and for authorizing actions from the HTTP API.
    // Should we add 1 more token, for more granular permissioning?
    #[serde(default, serialize_with = "serialize_optional_secret_string")]
    pub ilp_over_http_incoming_token: Option<SecretString>,
    /// The account's outgoing ILP over HTTP token
    /// This must match the ILP over HTTP incoming token on the peer's node if sending
    /// packets to that peer
    #[serde(default, serialize_with = "serialize_optional_secret_string")]
    pub ilp_over_http_outgoing_token: Option<SecretString>,
    /// The account's ILP over BTP URL (this is where packets are sent over WebSockets from your node)
    pub ilp_over_btp_url: Option<String>,
    /// The account's outgoing ILP over BTP token.
    /// This must match the ILP over BTP incoming token on the peer's node if exchanging
    /// packets with that peer.
    #[serde(default, serialize_with = "serialize_optional_secret_string")]
    pub ilp_over_btp_outgoing_token: Option<SecretString>,
    /// The account's incoming ILP over BTP token.
    /// This must match the ILP over BTP outgoing token on the peer's node if exchanging
    /// packets with that peer.
    #[serde(default, serialize_with = "serialize_optional_secret_string")]
    pub ilp_over_btp_incoming_token: Option<SecretString>,
    /// The threshold after which the balance service will trigger a settlement
    #[serde(default, deserialize_with = "optional_number_or_string")]
    pub settle_threshold: Option<i64>,
    /// The amount which the balance service will attempt to settle down to
    #[serde(default, deserialize_with = "optional_number_or_string")]
    pub settle_to: Option<i64>,
    /// The routing relation of the account
    pub routing_relation: Option<String>,
    /// The round trip time of the account (should be set depending on how
    /// well the network connectivity of the account and the node is)
    #[serde(default, deserialize_with = "optional_number_or_string")]
    pub round_trip_time: Option<u32>,
    /// The maximum amount the account can send per minute
    #[serde(default, deserialize_with = "optional_number_or_string")]
    pub amount_per_minute_limit: Option<u64>,
    /// The limit of packets the account can send per minute
    #[serde(default, deserialize_with = "optional_number_or_string")]
    pub packets_per_minute_limit: Option<u32>,
    /// The account's settlement engine URL. If a global engine url is configured
    /// for the account's asset code,  that will be used instead (even if the account is
    /// configured with a specific one)
    pub settlement_engine_url: Option<String>,
}

pub struct NodeApi<S, I, O, B, A: Account> {
    store: S,
    /// The admin's API token, used to make admin-only changes
    // TODO: Make this a SecretString
    admin_api_token: String,
    default_spsp_account: Option<Username>,
    incoming_handler: I,
    // The outgoing service is included so that the API can send outgoing
    // requests to specific accounts (namely ILDCP requests)
    outgoing_handler: O,
    // The BTP service is included here so that we can add a new client
    // connection when an account is added with BTP details
    btp: BtpOutgoingService<B, A>,
    /// Server secret used to instantiate SPSP/Stream connections
    server_secret: Bytes,
    node_version: Option<String>,
}

impl<S, I, O, B, A> NodeApi<S, I, O, B, A>
where
    S: NodeStore<Account = A>
        + AccountStore<Account = A>
        + AddressStore
        + HttpStore<Account = A>
        + BalanceStore
        + SettlementStore<Account = A>
        + StreamNotificationsStore<Account = A>
        + RouterStore
        + ExchangeRateStore,
    I: IncomingService<A> + Clone + Send + Sync + 'static,
    O: OutgoingService<A> + Clone + Send + Sync + 'static,
    B: OutgoingService<A> + Clone + Send + Sync + 'static,
    A: BtpAccount
        + CcpRoutingAccount
        + Account
        + HttpAccount
        + SettlementAccount
        + Serialize
        + Send
        + Sync
        + 'static,
{
    pub fn new(
        server_secret: Bytes,
        admin_api_token: String,
        store: S,
        incoming_handler: I,
        outgoing_handler: O,
        btp: BtpOutgoingService<B, A>,
    ) -> Self {
        NodeApi {
            store,
            admin_api_token,
            default_spsp_account: None,
            incoming_handler,
            outgoing_handler,
            btp,
            server_secret,
            node_version: None,
        }
    }

    /// Sets the default SPSP account. When SPSP payments are sent to the root domain,
    /// the payment pointer is resolved to <domain>/.well-known/pay. This value determines
    /// which account those payments will be sent to.
    pub fn default_spsp_account(&mut self, username: Username) -> &mut Self {
        self.default_spsp_account = Some(username);
        self
    }

    /// Sets the node version
    pub fn node_version(&mut self, version: String) -> &mut Self {
        self.node_version = Some(version);
        self
    }

    /// Returns a Warp Filter which exposes the accounts and admin APIs
    pub fn into_warp_filter(self) -> warp::filters::BoxedFilter<(impl warp::Reply,)> {
        routes::accounts_api(
            self.server_secret,
            self.admin_api_token.clone(),
            self.default_spsp_account,
            self.incoming_handler,
            self.outgoing_handler,
            self.btp,
            self.store.clone(),
        )
        .or(routes::node_settings_api(
            self.admin_api_token,
            self.node_version,
            self.store,
        ))
        .boxed()
    }

    /// Serves the API at the provided address
    pub async fn bind(self, addr: SocketAddr) {
        warp::serve(self.into_warp_filter()).bind(addr).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{self, json};

    #[test]
    fn number_or_string_deserialization() {
        #[derive(PartialEq, Deserialize, Debug)]
        struct One {
            #[serde(deserialize_with = "number_or_string")]
            val: u64,
        }
        assert_eq!(
            serde_json::from_str::<One>("{\"val\":1}").unwrap(),
            One { val: 1 }
        );
        assert_eq!(
            serde_json::from_str::<One>("{\"val\":\"1\"}").unwrap(),
            One { val: 1 }
        );

        assert!(serde_json::from_str::<One>("{\"val\":\"not-a-number\"}").is_err());
        assert!(serde_json::from_str::<One>("{\"val\":\"-1\"}").is_err());
    }

    #[test]
    fn optional_number_or_string_deserialization() {
        #[derive(PartialEq, Deserialize, Debug)]
        struct One {
            #[serde(deserialize_with = "optional_number_or_string")]
            val: Option<u64>,
        }
        assert_eq!(
            serde_json::from_str::<One>("{\"val\":1}").unwrap(),
            One { val: Some(1) }
        );
        assert_eq!(
            serde_json::from_str::<One>("{\"val\":\"1\"}").unwrap(),
            One { val: Some(1) }
        );
        assert!(serde_json::from_str::<One>("{}").is_err());

        #[derive(PartialEq, Deserialize, Debug)]
        struct Two {
            #[serde(default, deserialize_with = "optional_number_or_string")]
            val: Option<u64>,
        }
        assert_eq!(
            serde_json::from_str::<Two>("{\"val\":2}").unwrap(),
            Two { val: Some(2) }
        );
        assert_eq!(
            serde_json::from_str::<Two>("{\"val\":\"2\"}").unwrap(),
            Two { val: Some(2) }
        );
        assert_eq!(
            serde_json::from_str::<Two>("{}").unwrap(),
            Two { val: None }
        );
    }

    #[test]
    fn account_settings_deserialization() {
        let settings: AccountSettings = serde_json::from_value(json!({
            "ilp_over_http_url": "https://example.com/ilp",
            "ilp_over_http_incoming_token": "secret",
            "settle_to": 0,
            "settle_threshold": "1000",
        }))
        .unwrap();
        assert_eq!(settings.settle_threshold, Some(1000));
        assert_eq!(settings.settle_to, Some(0));
        assert_eq!(
            settings.ilp_over_http_url,
            Some("https://example.com/ilp".to_string())
        );
        assert!(settings.ilp_over_btp_url.is_none());
    }
}
