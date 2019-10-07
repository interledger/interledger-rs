use bytes::Bytes;
use futures::Future;
use interledger_http::{HttpAccount, HttpServer as IlpOverHttpServer, HttpStore};
use interledger_packet::Address;
use interledger_router::RouterStore;
use interledger_service::{Account, AddressStore, IncomingService, OutgoingService, Username};
use interledger_service_util::{BalanceStore, ExchangeRateStore};
use interledger_settlement::{SettlementAccount, SettlementStore};
use interledger_stream::StreamNotificationsStore;
use serde::{de, Deserialize, Serialize};
use std::{
    collections::HashMap,
    error::Error as StdError,
    fmt::{self, Display},
    net::SocketAddr,
    str::FromStr,
};
use warp::{self, Filter};
mod routes;
use interledger_btp::{BtpAccount, BtpOutgoingService};
use interledger_ccp::CcpRoutingAccount;
use secrecy::SecretString;
use url::Url;

pub(crate) mod http_retry;

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
        NumOrStr::Str(s) => T::from_str(&s)
            .map_err(de::Error::custom)
            .and_then(|n| Ok(Some(n))),
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

// TODO should the methods from this trait be split up and put into the
// traits that are more specific to what they're doing?
// One argument against doing that is that the NodeStore allows admin-only
// modifications to the values, whereas many of the other traits mostly
// read from the configured values.
pub trait NodeStore: AddressStore + Clone + Send + Sync + 'static {
    type Account: Account;

    fn insert_account(
        &self,
        account: AccountDetails,
    ) -> Box<dyn Future<Item = Self::Account, Error = ()> + Send>;

    fn delete_account(
        &self,
        id: <Self::Account as Account>::AccountId,
    ) -> Box<dyn Future<Item = Self::Account, Error = ()> + Send>;

    fn update_account(
        &self,
        id: <Self::Account as Account>::AccountId,
        account: AccountDetails,
    ) -> Box<dyn Future<Item = Self::Account, Error = ()> + Send>;

    fn modify_account_settings(
        &self,
        id: <Self::Account as Account>::AccountId,
        settings: AccountSettings,
    ) -> Box<dyn Future<Item = Self::Account, Error = ()> + Send>;

    // TODO limit the number of results and page through them
    fn get_all_accounts(&self) -> Box<dyn Future<Item = Vec<Self::Account>, Error = ()> + Send>;

    fn set_static_routes<R>(&self, routes: R) -> Box<dyn Future<Item = (), Error = ()> + Send>
    where
        R: IntoIterator<Item = (String, <Self::Account as Account>::AccountId)>;

    fn set_static_route(
        &self,
        prefix: String,
        account_id: <Self::Account as Account>::AccountId,
    ) -> Box<dyn Future<Item = (), Error = ()> + Send>;

    fn set_default_route(
        &self,
        account_id: <Self::Account as Account>::AccountId,
    ) -> Box<dyn Future<Item = (), Error = ()> + Send>;

    fn set_settlement_engines(
        &self,
        asset_to_url_map: impl IntoIterator<Item = (String, Url)>,
    ) -> Box<dyn Future<Item = (), Error = ()> + Send>;

    fn get_asset_settlement_engine(
        &self,
        asset_code: &str,
    ) -> Box<dyn Future<Item = Option<Url>, Error = ()> + Send>;
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
    pub ilp_over_http_incoming_token: Option<SecretString>,
    pub ilp_over_btp_incoming_token: Option<SecretString>,
    pub ilp_over_http_outgoing_token: Option<SecretString>,
    pub ilp_over_btp_outgoing_token: Option<SecretString>,
    pub ilp_over_http_url: Option<String>,
    pub ilp_over_btp_url: Option<String>,
    #[serde(default, deserialize_with = "optional_number_or_string")]
    pub settle_threshold: Option<i64>,
    // Note that this is intentionally an unsigned integer because users should
    // not be able to set the settle_to value to be negative (meaning the node
    // would pre-fund with the user)
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
    pub settle_threshold: Option<i64>,
    #[serde(default, deserialize_with = "optional_number_or_string")]
    pub settle_to: Option<u64>,
}

/// The Account type for the RedisStore.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AccountDetails {
    pub ilp_address: Option<Address>,
    pub username: Username,
    pub asset_code: String,
    #[serde(deserialize_with = "number_or_string")]
    pub asset_scale: u8,
    #[serde(default = "u64::max_value", deserialize_with = "number_or_string")]
    pub max_packet_amount: u64,
    #[serde(default, deserialize_with = "optional_number_or_string")]
    pub min_balance: Option<i64>,
    pub ilp_over_http_url: Option<String>,
    pub ilp_over_http_incoming_token: Option<SecretString>,
    pub ilp_over_http_outgoing_token: Option<SecretString>,
    pub ilp_over_btp_url: Option<String>,
    pub ilp_over_btp_outgoing_token: Option<SecretString>,
    pub ilp_over_btp_incoming_token: Option<SecretString>,
    #[serde(default, deserialize_with = "optional_number_or_string")]
    pub settle_threshold: Option<i64>,
    #[serde(default, deserialize_with = "optional_number_or_string")]
    pub settle_to: Option<i64>,
    pub routing_relation: Option<String>,
    #[serde(default, deserialize_with = "optional_number_or_string")]
    pub round_trip_time: Option<u32>,
    #[serde(default, deserialize_with = "optional_number_or_string")]
    pub amount_per_minute_limit: Option<u64>,
    #[serde(default, deserialize_with = "optional_number_or_string")]
    pub packets_per_minute_limit: Option<u32>,
    pub settlement_engine_url: Option<String>,
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum ApiError {
    AccountNotFound,
    BadRequest,
    InternalServerError,
    Unauthorized,
}

impl Display for ApiError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str(match self {
            ApiError::AccountNotFound => "Account not found",
            ApiError::BadRequest => "Bad request",
            ApiError::InternalServerError => "Internal server error",
            ApiError::Unauthorized => "Unauthorized",
        })
    }
}

impl StdError for ApiError {}

pub struct NodeApi<S, I, O, B, A: Account> {
    store: S,
    admin_api_token: String,
    default_spsp_account: Option<Username>,
    incoming_handler: I,
    // The outgoing service is included so that the API can send outgoing
    // requests to specific accounts (namely ILDCP requests)
    outgoing_handler: O,
    // The BTP service is included here so that we can add a new client
    // connection when an account is added with BTP details
    btp: BtpOutgoingService<B, A>,
    server_secret: Bytes,
}

impl<S, I, O, B, A> NodeApi<S, I, O, B, A>
where
    S: NodeStore<Account = A>
        + HttpStore<Account = A>
        + BalanceStore<Account = A>
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
        }
    }

    pub fn default_spsp_account(&mut self, username: Username) -> &mut Self {
        self.default_spsp_account = Some(username);
        self
    }

    pub fn into_warp_filter(self) -> warp::filters::BoxedFilter<(impl warp::Reply,)> {
        // POST /ilp is the route for ILP-over-HTTP
        let ilp_over_http = warp::path("ilp").and(warp::path::end()).and(
            IlpOverHttpServer::new(self.incoming_handler.clone(), self.store.clone()).as_filter(),
        );

        ilp_over_http
            .or(routes::accounts_api(
                self.server_secret,
                self.admin_api_token.clone(),
                self.default_spsp_account,
                self.incoming_handler,
                self.outgoing_handler,
                self.btp,
                self.store.clone(),
            ))
            .or(routes::node_settings_api(self.admin_api_token, self.store))
            .recover(|err: warp::Rejection| {
                if let Some(&err) = err.find_cause::<ApiError>() {
                    let code = match err {
                        ApiError::AccountNotFound => warp::http::StatusCode::NOT_FOUND,
                        ApiError::BadRequest => warp::http::StatusCode::BAD_REQUEST,
                        ApiError::InternalServerError => {
                            warp::http::StatusCode::INTERNAL_SERVER_ERROR
                        }
                        ApiError::Unauthorized => warp::http::StatusCode::UNAUTHORIZED,
                    };
                    Ok(warp::reply::with_status(warp::reply(), code))
                } else {
                    Err(err)
                }
            })
            .boxed()
    }

    pub fn bind(self, addr: SocketAddr) -> impl Future<Item = (), Error = ()> {
        warp::serve(self.into_warp_filter()).bind(addr)
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
