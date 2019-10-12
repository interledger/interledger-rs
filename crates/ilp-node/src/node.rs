use bytes::Bytes;
use futures::{future::result, Future};
use hex::FromHex;
#[doc(hidden)]
pub use interledger::api::AccountDetails;
pub use interledger::service_util::ExchangeRateProvider;

use interledger::{
    api::{NodeApi, NodeStore},
    btp::{connect_client, create_btp_service_and_filter, BtpStore},
    ccp::CcpRouteManagerBuilder,
    http::HttpClientService,
    ildcp::IldcpService,
    packet::Address,
    packet::{ErrorCode, RejectBuilder},
    router::Router,
    service::{outgoing_service_fn, Account as AccountTrait, OutgoingRequest, Username},
    service_util::{
        BalanceService, EchoService, ExchangeRateFetcher, ExchangeRateService,
        ExpiryShortenerService, MaxPacketAmountService, RateLimitService, ValidatorService,
    },
    settlement::{SettlementApi, SettlementMessageService},
    store_redis::{Account, AccountId, ConnectionInfo, IntoConnectionInfo, RedisStoreBuilder},
    stream::StreamReceiverService,
};
use lazy_static::lazy_static;
use log::{debug, error, info, trace};
use ring::{
    hmac,
    rand::{SecureRandom, SystemRandom},
};
use serde::{de::Error as DeserializeError, Deserialize, Deserializer};
use std::{convert::TryFrom, net::SocketAddr, str, str::FromStr, time::Duration};
use tokio::{net::TcpListener, spawn};
use url::Url;
use warp::{self, Filter};

static REDIS_SECRET_GENERATION_STRING: &str = "ilp_redis_secret";
static DEFAULT_REDIS_URL: &str = "redis://127.0.0.1:6379";
lazy_static! {
    static ref DEFAULT_ILP_ADDRESS: Address = Address::from_str("local.host").unwrap();
}

fn default_settlement_api_bind_address() -> SocketAddr {
    SocketAddr::from(([127, 0, 0, 1], 7771))
}
fn default_http_bind_address() -> SocketAddr {
    SocketAddr::from(([127, 0, 0, 1], 7770))
}
fn default_redis_url() -> ConnectionInfo {
    DEFAULT_REDIS_URL.into_connection_info().unwrap()
}
fn default_exchange_rate_poll_interval() -> u64 {
    60000
}

fn deserialize_optional_address<'de, D>(deserializer: D) -> Result<Option<Address>, D::Error>
where
    D: Deserializer<'de>,
{
    if let Ok(address) = Bytes::deserialize(deserializer) {
        Address::try_from(address)
            .map(Some)
            .map_err(|err| DeserializeError::custom(format!("Invalid address: {:?}", err)))
    } else {
        Ok(None)
    }
}

fn deserialize_32_bytes_hex<'de, D>(deserializer: D) -> Result<[u8; 32], D::Error>
where
    D: Deserializer<'de>,
{
    <[u8; 32]>::from_hex(String::deserialize(deserializer)?).map_err(|err| {
        DeserializeError::custom(format!(
            "Invalid hex value (must be 32 hex-encoded bytes): {:?}",
            err
        ))
    })
}

fn deserialize_optional_username<'de, D>(deserializer: D) -> Result<Option<Username>, D::Error>
where
    D: Deserializer<'de>,
{
    if let Ok(username) = String::deserialize(deserializer) {
        Username::from_str(&username)
            .map(Some)
            .map_err(|err| DeserializeError::custom(format!("Invalid username: {:?}", err)))
    } else {
        Ok(None)
    }
}

fn deserialize_redis_connection<'de, D>(deserializer: D) -> Result<ConnectionInfo, D::Error>
where
    D: Deserializer<'de>,
{
    Url::parse(&String::deserialize(deserializer)?)
        .map_err(|err| DeserializeError::custom(format!("Invalid URL: {:?}", err)))?
        .into_connection_info()
        .map_err(|err| {
            DeserializeError::custom(format!(
                "Error converting into Redis connection info: {:?}",
                err
            ))
        })
}

/// An all-in-one Interledger node that includes sender and receiver functionality,
/// a connector, and a management API. The node uses Redis for persistence.
#[derive(Deserialize, Clone)]
pub struct InterledgerNode {
    /// ILP address of the node
    #[serde(deserialize_with = "deserialize_optional_address")]
    #[serde(default)]
    pub ilp_address: Option<Address>,
    /// Root secret used to derive encryption keys
    #[serde(deserialize_with = "deserialize_32_bytes_hex")]
    pub secret_seed: [u8; 32],
    /// HTTP Authorization token for the node admin (sent as a Bearer token)
    pub admin_auth_token: String,
    /// Redis URI (for example, "redis://127.0.0.1:6379" or "unix:/tmp/redis.sock")
    #[serde(
        deserialize_with = "deserialize_redis_connection",
        default = "default_redis_url",
        alias = "redis_url"
    )]
    pub redis_connection: ConnectionInfo,
    /// IP address and port to listen for HTTP connections
    /// This is used for both the API and ILP over HTTP packets
    #[serde(default = "default_http_bind_address")]
    pub http_bind_address: SocketAddr,
    /// IP address and port to listen for the Settlement Engine API
    #[serde(default = "default_settlement_api_bind_address")]
    pub settlement_api_bind_address: SocketAddr,
    /// When SPSP payments are sent to the root domain, the payment pointer is resolved
    /// to <domain>/.well-known/pay. This value determines which account those payments
    /// will be sent to.
    #[serde(default, deserialize_with = "deserialize_optional_username")]
    pub default_spsp_account: Option<Username>,
    /// Interval, defined in milliseconds, on which the node will broadcast routing
    /// information to other nodes using CCP. Defaults to 30000ms (30 seconds).
    pub route_broadcast_interval: Option<u64>,
    /// Interval, defined in milliseconds, on which the node will poll the exchange rate provider.
    /// Defaults to 60000ms (60 seconds).
    #[serde(default = "default_exchange_rate_poll_interval")]
    pub exchange_rate_poll_interval: u64,
    /// API to poll for exchange rates. Currently the supported options are:
    /// - [CoinCap](https://docs.coincap.io)
    /// - [CryptoCompare](https://cryptocompare.com) (note this requires an API key)
    /// If this value is not set, the node will not poll for exchange rates and will
    /// instead use the rates configured via the HTTP API.
    pub exchange_rate_provider: Option<ExchangeRateProvider>,
    // Spread, as a fraction, to add on top of the exchange rate.
    // This amount is kept as the node operator's profit, or may cover
    // fluctuations in exchange rates.
    // For example, take an incoming packet with an amount of 100. If the
    // exchange rate is 1:2 and the spread is 0.01, the amount on the
    // outgoing packet would be 198 (instead of 200 without the spread).
    pub exchange_rate_spread: f64,
}

impl InterledgerNode {
    /// Returns a future that runs the Interledger Node
    // TODO when a BTP connection is made, insert a outgoing HTTP entry into the Store to tell other
    // connector instances to forward packets for that account to us
    pub fn serve(&self) -> impl Future<Item = (), Error = ()> {
        let redis_secret = generate_redis_secret(&self.secret_seed);
        let secret_seed = Bytes::from(&self.secret_seed[..]);
        let http_bind_address = self.http_bind_address;
        let settlement_api_bind_address = self.settlement_api_bind_address;
        let ilp_address = if let Some(address) = &self.ilp_address {
            address.clone()
        } else {
            DEFAULT_ILP_ADDRESS.clone()
        };
        let ilp_address_clone = ilp_address.clone();
        let ilp_address_clone2 = ilp_address.clone();
        let admin_auth_token = self.admin_auth_token.clone();
        let default_spsp_account = self.default_spsp_account.clone();
        let redis_addr = self.redis_connection.addr.clone();
        let route_broadcast_interval = self.route_broadcast_interval;
        let exchange_rate_provider = self.exchange_rate_provider.clone();
        let exchange_rate_poll_interval = self.exchange_rate_poll_interval;
        let exchange_rate_spread = self.exchange_rate_spread;

        debug!(
            "Starting Interledger node with ILP address: {}",
            ilp_address
        );

        Box::new(RedisStoreBuilder::new(self.redis_connection.clone(), redis_secret)
        .node_ilp_address(ilp_address.clone())
        .connect()
        .map_err(move |err| error!("Error connecting to Redis: {:?} {:?}", redis_addr, err))
        .and_then(move |store| {
                store.clone().get_btp_outgoing_accounts()
                .map_err(|_| error!("Error getting accounts"))
                .and_then(move |btp_accounts| {
                    let outgoing_service =
                        outgoing_service_fn(move |request: OutgoingRequest<Account>| {
                            error!("No route found for outgoing account {} (id: {})", request.to.username(), request.to.id());
                            trace!("Rejecting request to account {:?}, prepare packet: {:?}", request.to, request.prepare);
                            Err(RejectBuilder {
                                code: ErrorCode::F02_UNREACHABLE,
                                message: &format!(
                                    "No outgoing route for account: {} (ILP address of the Prepare packet: {:?})",
                                    request.to.id(),
                                    request.prepare.destination(),
                                )
                                .as_bytes(),
                                triggered_by: Some(&ilp_address_clone),
                                data: &[],
                            }
                            .build())
                        });

                    // Connect to all of the accounts that have outgoing ilp_over_btp_urls configured
                    // but don't fail if we are unable to connect
                    // TODO try reconnecting to those accounts later
                    connect_client(ilp_address_clone2.clone(), btp_accounts, false, outgoing_service).and_then(
                        move |btp_client_service| {
                            let (btp_server_service, btp_filter) = create_btp_service_and_filter(ilp_address_clone2, store.clone(), btp_client_service.clone());
                            let btp = btp_client_service.clone();

                            // The BTP service is both an Incoming and Outgoing one so we pass it first as the Outgoing
                            // service to others like the router and then call handle_incoming on it to set up the incoming handler
                            let outgoing_service = btp_server_service.clone();
                            let outgoing_service = ValidatorService::outgoing(
                                store.clone(),
                                outgoing_service
                            );
                            let outgoing_service = HttpClientService::new(
                                store.clone(),
                                outgoing_service,
                            );

                            // Note: the expiry shortener must come after the Validator so that the expiry duration
                            // is shortened before we check whether there is enough time left
                            let outgoing_service =
                                ExpiryShortenerService::new(outgoing_service);
                            let outgoing_service = StreamReceiverService::new(
                                secret_seed.clone(),
                                store.clone(),
                                outgoing_service,
                            );
                            let outgoing_service = BalanceService::new(
                                store.clone(),
                                outgoing_service,
                            );
                            let outgoing_service = ExchangeRateService::new(
                                exchange_rate_spread,
                                store.clone(),
                                outgoing_service,
                            );

                            // Set up the Router and Routing Manager
                            let incoming_service = Router::new(
                                store.clone(),
                                outgoing_service.clone()
                            );
                            let mut ccp_builder = CcpRouteManagerBuilder::new(
                                ilp_address.clone(),
                                store.clone(),
                                outgoing_service.clone(),
                                incoming_service,
                            );
                            ccp_builder.ilp_address(ilp_address.clone());
                            if let Some(ms) = route_broadcast_interval {
                                ccp_builder.broadcast_interval(ms);
                            }
                            let incoming_service = ccp_builder.to_service();
                            let incoming_service = EchoService::new(store.clone(), incoming_service);
                            let incoming_service = SettlementMessageService::new(incoming_service);
                            let incoming_service = IldcpService::new(incoming_service);
                            let incoming_service =
                                MaxPacketAmountService::new(
                                    store.clone(),
                                    incoming_service
                            );
                            let incoming_service =
                                ValidatorService::incoming(store.clone(), incoming_service);
                            let incoming_service = RateLimitService::new(
                                store.clone(),
                                incoming_service,
                            );

                            // Handle incoming packets sent via BTP
                            btp_server_service.handle_incoming(incoming_service.clone());
                            btp_client_service.handle_incoming(incoming_service.clone());

                            // Node HTTP API
                            let mut api = NodeApi::new(
                                secret_seed,
                                admin_auth_token,
                                store.clone(),
                                incoming_service.clone(),
                                outgoing_service.clone(),
                                btp.clone(),
                            );
                            if let Some(username) = default_spsp_account {
                                api.default_spsp_account(username);
                            }

                            // Mount the BTP endpoint at /ilp/btp
                            let btp_endpoint = warp::path("ilp")
                                .and(warp::path("btp"))
                                .and(warp::path::end())
                                .and(btp_filter);
                            // Note that other endpoints added to the API must come first
                            // because the API includes error handling and consumes the request.
                            // TODO should we just make BTP part of the API?
                            let api = btp_endpoint.or(api.into_warp_filter()).with(warp::log("interledger-api")).boxed();
                            spawn(warp::serve(api).bind(http_bind_address));

                            // Settlement API
                            let settlement_api = SettlementApi::new(
                                store.clone(),
                                outgoing_service.clone(),
                            );
                            let listener = TcpListener::bind(&settlement_api_bind_address)
                                .expect("Unable to bind to Settlement API address");
                            info!("Settlement API listening on: {}", settlement_api_bind_address);
                            spawn(settlement_api.serve(listener.incoming()));

                            // Exchange Rate Polling
                            if let Some(provider) = exchange_rate_provider {
                                let exchange_rate_fetcher = ExchangeRateFetcher::new(provider, store.clone());
                                exchange_rate_fetcher.spawn_interval(Duration::from_millis(exchange_rate_poll_interval));
                            } else {
                                debug!("Not using exchange rate provider. Rates must be set via the HTTP API");
                            }

                            Ok(())
                        },
                    )
                })
        }))
    }

    /// Run the node on the default Tokio runtime
    pub fn run(&self) {
        tokio_run(self.serve());
    }

    #[doc(hidden)]
    #[allow(dead_code)]
    pub fn insert_account(
        &self,
        account: AccountDetails,
    ) -> impl Future<Item = AccountId, Error = ()> {
        let redis_secret = generate_redis_secret(&self.secret_seed);
        result(self.redis_connection.clone().into_connection_info())
            .map_err(|err| error!("Invalid Redis connection details: {:?}", err))
            .and_then(move |redis_url| RedisStoreBuilder::new(redis_url, redis_secret).connect())
            .map_err(|err| error!("Error connecting to Redis: {:?}", err))
            .and_then(move |store| {
                store
                    .insert_account(account)
                    .map_err(|_| error!("Unable to create account"))
                    .and_then(|account| {
                        debug!("Created account: {}", account.id());
                        Ok(account.id())
                    })
            })
    }
}

fn generate_redis_secret(secret_seed: &[u8; 32]) -> [u8; 32] {
    let mut redis_secret: [u8; 32] = [0; 32];
    let sig = hmac::sign(
        &hmac::Key::new(hmac::HMAC_SHA256, secret_seed),
        REDIS_SECRET_GENERATION_STRING.as_bytes(),
    );
    redis_secret.copy_from_slice(sig.as_ref());
    redis_secret
}

#[doc(hidden)]
pub fn tokio_run(fut: impl Future<Item = (), Error = ()> + Send + 'static) {
    let mut runtime = tokio::runtime::Builder::new()
        // Don't swallow panics
        .panic_handler(|err| std::panic::resume_unwind(err))
        .name_prefix("interledger-rs-worker-")
        .build()
        .expect("failed to start new runtime");

    runtime.spawn(fut);
    runtime.shutdown_on_idle().wait().unwrap();
}

#[doc(hidden)]
#[allow(dead_code)]
pub fn random_secret() -> [u8; 32] {
    let mut bytes: [u8; 32] = [0; 32];
    SystemRandom::new().fill(&mut bytes).unwrap();
    bytes
}
