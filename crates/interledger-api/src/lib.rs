use bytes::Bytes;
use futures::Future;
use interledger_http::{HttpAccount, HttpServer as IlpOverHttpServer, HttpStore};
use interledger_packet::Address;
use interledger_router::RouterStore;
use interledger_service::{Account, AddressStore, IncomingService, OutgoingService, Username};
use interledger_service_util::{BalanceStore, ExchangeRateStore};
use interledger_settlement::{SettlementAccount, SettlementStore};
use interledger_stream::StreamNotificationsStore;
use serde::{Deserialize, Serialize};
use std::{
    error::Error as StdError,
    fmt::{self, Display},
    net::SocketAddr,
};
use warp::{self, Filter};
mod routes;
use interledger_btp::{BtpAccount, BtpOutgoingService};
use interledger_ccp::CcpRoutingAccount;

pub(crate) mod http_retry;

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
}

/// AccountSettings is a subset of the user parameters defined in
/// AccountDetails. Its purpose is to allow a user to modify certain of their
/// parameters which they may want to re-configure in the future, such as their
/// tokens (which act as passwords), their settlement frequency preferences, or
/// their HTTP/BTP endpoints, since they may change their network configuration.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AccountSettings {
    // TODO these should be Secret types
    pub http_incoming_token: Option<String>,
    pub btp_incoming_token: Option<String>,
    pub http_outgoing_token: Option<String>,
    pub btp_outgoing_token: Option<String>,
    pub http_endpoint: Option<String>,
    pub btp_uri: Option<String>,
    pub settle_threshold: Option<i64>,
    // Note that this is intentionally an unsigned integer because users should
    // not be able to set the settle_to value to be negative (meaning the node
    // would pre-fund with the user)
    pub settle_to: Option<u64>,
}

/// The Account type for the RedisStore.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AccountDetails {
    pub ilp_address: Option<Address>,
    pub username: Username,
    pub asset_code: String,
    pub asset_scale: u8,
    #[serde(default = "u64::max_value")]
    pub max_packet_amount: u64,
    pub min_balance: Option<i64>,
    pub http_endpoint: Option<String>,
    // TODO use Secrets here
    pub http_incoming_token: Option<String>,
    pub http_outgoing_token: Option<String>,
    pub btp_uri: Option<String>,
    pub btp_incoming_token: Option<String>,
    pub settle_threshold: Option<i64>,
    pub settle_to: Option<i64>,
    pub routing_relation: Option<String>,
    pub round_trip_time: Option<u32>,
    pub amount_per_minute_limit: Option<u64>,
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
            .with(warp::log("interledger-api"))
            .boxed()
    }

    pub fn bind(self, addr: SocketAddr) -> impl Future<Item = (), Error = ()> {
        warp::serve(self.into_warp_filter()).bind(addr)
    }
}
