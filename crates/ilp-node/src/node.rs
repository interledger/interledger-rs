use cfg_if::cfg_if;

#[cfg(feature = "google-pubsub")]
use crate::instrumentation::google_pubsub::{create_google_pubsub_wrapper, PubsubConfig};

cfg_if! {
    if #[cfg(feature = "monitoring")] {
        use tracing_subscriber::{
            filter::EnvFilter,
            fmt::{time::ChronoUtc, Subscriber},
        };
        use interledger::errors::ApiError;
        use secrecy::{ExposeSecret, SecretString};
        use tracing_futures::Instrument;
        use tracing::debug_span;
        use crate::instrumentation::{
            metrics::{incoming_metrics, outgoing_metrics},
            prometheus::{serve_prometheus, PrometheusConfig},
            trace::{trace_forwarding, trace_incoming, trace_outgoing},
        };
        use interledger::service::IncomingService;
        use futures::FutureExt;
    }
}

#[cfg(any(feature = "monitoring", feature = "google-pubsub"))]
use interledger::service::OutgoingService;

use bytes::Bytes;
use futures::TryFutureExt;
use hex::FromHex;
use interledger::{
    api::{NodeApi, NodeStore},
    btp::{btp_service_as_filter, connect_client, BtpOutgoingService, BtpStore},
    ccp::{CcpRouteManagerBuilder, CcpRoutingAccount, CcpRoutingStore, RoutingRelation},
    errors::*,
    http::{HttpClientService, HttpServer as IlpOverHttpServer, HttpStore},
    ildcp::IldcpService,
    packet::Address,
    packet::{ErrorCode, RejectBuilder},
    router::{Router, RouterStore},
    service::{
        outgoing_service_fn, Account as AccountTrait, AccountStore, AddressStore, OutgoingRequest,
        Username,
    },
    service_util::{
        BalanceStore, EchoService, ExchangeRateFetcher, ExchangeRateService, ExchangeRateStore,
        ExpiryShortenerService, MaxPacketAmountService, RateLimitService, RateLimitStore,
        ValidatorService,
    },
    settlement::{
        api::{create_settlements_filter, SettlementMessageService},
        core::{
            idempotency::IdempotentStore,
            types::{LeftoversStore, SettlementStore},
        },
    },
    store::account::Account,
    stream::{StreamNotificationsStore, StreamReceiverService},
};
use num_bigint::BigUint;
use once_cell::sync::Lazy;
use serde::{de::Error as DeserializeError, Deserialize, Deserializer};
use std::{convert::TryFrom, net::SocketAddr, str, str::FromStr, time::Duration};
use tokio::spawn;
use tracing::{debug, error, info};
use url::Url;
use uuid::Uuid;
use warp::{self, Filter};

#[cfg(feature = "redis")]
use crate::redis_store::*;
#[cfg(feature = "balance-tracking")]
use interledger::service_util::BalanceService;

#[doc(hidden)]
pub use interledger::service_util::ExchangeRateProvider;

static DEFAULT_ILP_ADDRESS: Lazy<Address> = Lazy::new(|| Address::from_str("local.host").unwrap());

fn default_settlement_api_bind_address() -> SocketAddr {
    SocketAddr::from(([127, 0, 0, 1], 7771))
}
fn default_http_bind_address() -> SocketAddr {
    SocketAddr::from(([127, 0, 0, 1], 7770))
}
// We allow unreachable code on the below function because there must always be exactly one default
// regardless of how many data sources the crate is compiled to support,
// but we don't know which will be enabled or in which quantities or configurations.
// This return-based pattern effectively gives us fallthrough behavior.
#[allow(unreachable_code)]
fn default_database_url() -> String {
    #[cfg(feature = "redis")]
    return default_redis_url();
    panic!("no backing store configured")
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

/// Configuration for calculating exchange rates between various pairs.
#[derive(Deserialize, Clone, Default)]
pub struct ExchangeRateConfig {
    /// Interval, defined in milliseconds, on which the node will poll the exchange rate provider.
    /// Defaults to 60000ms (60 seconds).
    #[serde(default = "ExchangeRateConfig::default_poll_interval")]
    pub poll_interval: u64,
    /// The number of consecutive failed polls to the exchange rate provider
    /// that the connector will tolerate before invalidating the exchange rate cache.
    #[serde(default = "ExchangeRateConfig::default_poll_failure_tolerance")]
    pub poll_failure_tolerance: u32,
    /// API to poll for exchange rates. Currently the supported options are:
    /// - [CoinCap](https://docs.coincap.io)
    /// - [CryptoCompare](https://cryptocompare.com) (note this requires an API key)
    /// If this value is not set, the node will not poll for exchange rates and will
    /// instead use the rates configured via the HTTP API.
    #[serde(default)]
    pub provider: Option<ExchangeRateProvider>,
    /// Spread, as a fraction, to add on top of the exchange rate.
    /// This amount is kept as the node operator's profit, or may cover
    /// fluctuations in exchange rates.
    /// For example, take an incoming packet with an amount of 100. If the
    /// exchange rate is 1:2 and the spread is 0.01, the amount on the
    /// outgoing packet would be 198 (instead of 200 without the spread).
    #[serde(default)]
    pub spread: f64,
}

impl ExchangeRateConfig {
    fn default_poll_interval() -> u64 {
        60_000
    }
    fn default_poll_failure_tolerance() -> u32 {
        5
    }
}

/// An all-in-one Interledger node that includes sender and receiver functionality,
/// a connector, and a management API.
/// Will connect to the database at the given URL; see the crate features defined in
/// Cargo.toml to see a list of all supported stores.
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
    /// Data store URI (for example, "redis://127.0.0.1:6379" or "redis+unix:/tmp/redis.sock")
    #[serde(
        default = "default_database_url",
        // temporary alias for backwards compatibility
        alias = "redis_url"
    )]
    pub database_url: String,
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
    #[serde(default)]
    /// Configuration for calculating exchange rates between various pairs.
    pub exchange_rate: ExchangeRateConfig,
    /// Configuration for [Prometheus](https://prometheus.io) metrics collection.
    /// If this configuration is not provided, the node will not collect metrics.
    /// Needs the feature flag "monitoring" to be enabled
    #[cfg(feature = "monitoring")]
    #[serde(default)]
    pub prometheus: Option<PrometheusConfig>,
    #[cfg(feature = "google-pubsub")]
    pub google_pubsub: Option<PubsubConfig>,
}

impl InterledgerNode {
    /// Returns a future that runs the Interledger.rs Node.
    ///
    /// If the Prometheus configuration was provided, it will
    /// also run the Prometheus metrics server on the given address.
    // TODO when a BTP connection is made, insert a outgoing HTTP entry into the Store to tell other
    // connector instances to forward packets for that account to us
    pub async fn serve(self) -> Result<(), ()> {
        cfg_if! {
            if #[cfg(feature = "monitoring")] {
                let f = futures::future::join(serve_prometheus(self.clone()), self.serve_node()).then(
                    |r| async move {
                        if r.0.is_ok() || r.1.is_ok() {
                            Ok(())
                        } else {
                            Err(())
                        }
                    },
                );
            } else {
                let f = self.serve_node();
            }
        }

        f.await
    }

    async fn serve_node(self) -> Result<(), ()> {
        let ilp_address = if let Some(address) = &self.ilp_address {
            address.clone()
        } else {
            DEFAULT_ILP_ADDRESS.clone()
        };

        // TODO: store a Url directly in InterledgerNode rather than a String?
        let database_url = match Url::parse(&self.database_url) {
            Ok(url) => url,
            Err(e) => {
                error!(
                    "The string '{}' could not be parsed as a URL: {}",
                    &self.database_url, e
                );
                return Err(());
            }
        };

        match database_url.scheme() {
            #[cfg(feature = "redis")]
            "redis" | "redis+unix" => serve_redis_node(self, ilp_address).await,
            other => {
                error!("unsupported data source scheme: {}", other);
                Err(())
            }
        }
    }

    #[allow(clippy::cognitive_complexity)]
    pub(crate) async fn chain_services<S>(self, store: S, ilp_address: Address) -> Result<(), ()>
    where
        S: NodeStore<Account = Account>
            + AddressStore
            + BtpStore<Account = Account>
            + HttpStore<Account = Account>
            + StreamNotificationsStore<Account = Account>
            + BalanceStore
            + SettlementStore<Account = Account>
            + ExchangeRateStore
            + BalanceStore
            + SettlementStore<Account = Account>
            + RouterStore<Account = Account>
            + CcpRoutingStore<Account = Account>
            + RateLimitStore<Account = Account>
            + LeftoversStore<AccountId = Uuid, AssetType = BigUint>
            + IdempotentStore
            + AccountStore<Account = Account>
            + Clone
            + Send
            + Sync
            + 'static,
    {
        debug!(target: "interledger-node",
            "Starting Interledger node with ILP address: {}",
            ilp_address
        );

        let secret_seed = Bytes::from(&self.secret_seed[..]);
        let http_bind_address = self.http_bind_address;
        let settlement_api_bind_address = self.settlement_api_bind_address;
        let ilp_address_clone = ilp_address.clone();
        let ilp_address_clone2 = ilp_address.clone();
        let admin_auth_token = self.admin_auth_token.clone();
        let default_spsp_account = self.default_spsp_account.clone();
        let route_broadcast_interval = self.route_broadcast_interval;
        let exchange_rate_provider = self.exchange_rate.provider.clone();
        let exchange_rate_poll_interval = self.exchange_rate.poll_interval;
        let exchange_rate_poll_failure_tolerance = self.exchange_rate.poll_failure_tolerance;
        let exchange_rate_spread = self.exchange_rate.spread;
        #[cfg(feature = "google-pubsub")]
        let google_pubsub = self.google_pubsub.clone();

        let btp_accounts = store
            .get_btp_outgoing_accounts()
            .map_err(|_| error!(target: "interledger-node", "Error getting accounts"))
            .await?;

        let outgoing_service = outgoing_service_fn(move |request: OutgoingRequest<Account>| {
            // Don't log anything for failed route updates sent to child accounts
            // because there's a good chance they'll be offline
            if request.prepare.destination().scheme() != "peer"
                || request.to.routing_relation() != RoutingRelation::Child
            {
                error!(target: "interledger-node", "No route found for outgoing request");
            }
            Err(RejectBuilder {
                code: ErrorCode::F02_UNREACHABLE,
                message: &format!(
                    // TODO we might not want to expose the internal account ID in the error
                    "No outgoing route for account: {} (ILP address of the Prepare packet: {})",
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
        let btp_client_service = connect_client(
            ilp_address_clone2.clone(),
            btp_accounts,
            false,
            outgoing_service,
        )
        .map_err(|err| error!("{}", err))
        .await?;
        let btp_server_service =
            BtpOutgoingService::new(ilp_address_clone2, btp_client_service.clone());
        let btp_server_service_clone = btp_server_service.clone();
        let btp = btp_client_service.clone();

        // The BTP service is both an Incoming and Outgoing one so we pass it first as the Outgoing
        // service to others like the router and then call handle_incoming on it to set up the incoming handler
        let outgoing_service = btp_server_service.clone();
        let outgoing_service = HttpClientService::new(store.clone(), outgoing_service);

        #[cfg(feature = "monitoring")]
        let outgoing_service = outgoing_service.wrap(outgoing_metrics);

        // Note: the expiry shortener must come after the Validator so that the expiry duration
        // is shortened before we check whether there is enough time left
        let outgoing_service = ValidatorService::outgoing(store.clone(), outgoing_service);
        let outgoing_service = ExpiryShortenerService::new(outgoing_service);
        let outgoing_service =
            StreamReceiverService::new(secret_seed.clone(), store.clone(), outgoing_service);
        #[cfg(feature = "balance-tracking")]
        let outgoing_service = BalanceService::new(store.clone(), outgoing_service);
        let outgoing_service =
            ExchangeRateService::new(exchange_rate_spread, store.clone(), outgoing_service);

        #[cfg(feature = "google-pubsub")]
        let outgoing_service = outgoing_service.wrap(create_google_pubsub_wrapper(google_pubsub));

        // Add tracing to add the outgoing request details to the incoming span
        cfg_if! {
            if #[cfg(feature = "monitoring")] {
                let outgoing_service_fwd = outgoing_service
                    .clone()
                    .wrap(trace_forwarding);
            } else {
                let outgoing_service_fwd = outgoing_service.clone();
            }
        }

        // Set up the Router and Routing Manager
        let incoming_service = Router::new(store.clone(), outgoing_service_fwd);

        // Add tracing to track the outgoing request details
        #[cfg(feature = "monitoring")]
        let outgoing_service = outgoing_service.wrap(trace_outgoing).in_current_span();

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
        let incoming_service = MaxPacketAmountService::new(store.clone(), incoming_service);
        let incoming_service = ValidatorService::incoming(store.clone(), incoming_service);
        let incoming_service = RateLimitService::new(store.clone(), incoming_service);

        // Add tracing to track the incoming request details
        #[cfg(feature = "monitoring")]
        let incoming_service = incoming_service
            .wrap(trace_incoming)
            .in_current_span()
            .wrap(incoming_metrics);

        // Handle incoming packets sent via BTP
        cfg_if! {
            if #[cfg(feature = "monitoring")] {
                let incoming_service_btp = incoming_service
                    .clone()
                    .wrap(|request, mut next| async move {
                        let btp = debug_span!(target: "interledger-node", "btp");
                        let _btp_scope = btp.enter();
                        next.handle_request(request).in_current_span().await
                    })
                    .in_current_span();
            } else {
                let incoming_service_btp = incoming_service.clone();
            }
        }

        btp_server_service
            .handle_incoming(incoming_service_btp.clone())
            .await;

        btp_client_service
            .handle_incoming(incoming_service_btp)
            .await;

        cfg_if! {
            if #[cfg(feature = "monitoring")] {
                let incoming_service_api = incoming_service
                    .clone()
                    .wrap(|request, mut next| async move {
                        let api = debug_span!(target: "interledger-node", "api");
                        let _api_scope = api.enter();
                        next.handle_request(request).in_current_span().await
                    })
                    .in_current_span();
            } else {
                let incoming_service_api = incoming_service.clone();
            }
        }

        // Node HTTP API
        let mut api = NodeApi::new(
            bytes05::Bytes::copy_from_slice(secret_seed.as_ref()),
            admin_auth_token,
            store.clone(),
            incoming_service_api.clone(),
            outgoing_service.clone(),
            btp.clone(), // btp client service!
        );
        if let Some(username) = default_spsp_account {
            api.default_spsp_account(username);
        }
        api.node_version(env!("CARGO_PKG_VERSION").to_string());

        cfg_if! {
            if #[cfg(feature = "monitoring")] {
                let incoming_service_http = incoming_service
                    .clone()
                    .wrap(|request, mut next| async move {
                        let http = debug_span!(target: "interledger-node", "http");
                        let _http_scope = http.enter();
                        next.handle_request(request).in_current_span().await
                    })
                    .in_current_span();
            } else {
                let incoming_service_http = incoming_service.clone();
            }
        }

        // add an API of ILP over HTTP and add rejection handler
        let api = api
            .into_warp_filter()
            .or(IlpOverHttpServer::new(incoming_service_http, store.clone()).as_filter())
            .or(btp_service_as_filter(
                btp_server_service_clone,
                store.clone(),
            ));

        // If monitoring is enabled, run a tracing subscriber
        // and expose a new endpoint at /tracing-level which allows
        // changing the tracing level by administrators
        cfg_if! {
            if #[cfg(feature = "monitoring")] {
                let builder = Subscriber::builder()
                    .with_timer(ChronoUtc::rfc3339())
                    .with_env_filter(EnvFilter::from_default_env())
                    .with_filter_reloading();
                let handle = builder.reload_handle();
                builder.try_init().unwrap_or(());

                let admin_auth_token = self.admin_auth_token.clone();
                let api = {
                    let adjust_tracing = warp::put()
                        .and(warp::path("tracing-level"))
                        .and(warp::path::end())
                        .and(warp::header::<SecretString>("authorization"))
                        .and(warp::body::bytes())
                        .and_then(
                            move |auth_header: SecretString, new_level: bytes05::Bytes| {
                                let handle = handle.clone();
                                let admin_auth_header = format!("Bearer {}", admin_auth_token);
                                async move {
                                    if auth_header.expose_secret().as_str() != admin_auth_header {
                                        return Err(ApiError::unauthorized()
                                            .detail("invalid admin auth token")
                                            .into());
                                    }
                                    let new_level = std::str::from_utf8(new_level.as_ref()).map_err(|_| {
                                        ApiError::bad_request().detail("invalid utf-8 body provided")
                                    })?;
                                    let new_tracing_level = new_level
                                        .parse::<tracing_subscriber::filter::EnvFilter>()
                                        .map_err(|_| {
                                            ApiError::bad_request().detail("could not parse body as log level")
                                        })?;
                                    handle.reload(new_tracing_level).map_err(|err| {
                                        ApiError::internal_server_error()
                                            .detail(format!("could not apply new log level {}", err))
                                    })?;
                                    debug!(target: "interledger-node", "Logging level adjusted to {}", new_level);
                                    Ok::<String, warp::Rejection>(format!(
                                        "Logging level changed to: {}",
                                        new_level
                                    ))
                                }
                            },
                        );
                    api.or(adjust_tracing)
                };
            }
        }

        let api = api
            .recover(default_rejection_handler)
            .with(warp::log("interledger-api"))
            .boxed();

        info!(target: "interledger-node", "Interledger.rs node HTTP API listening on: {}", http_bind_address);
        spawn(warp::serve(api).bind(http_bind_address));

        // Settlement API
        let settlement_api = create_settlements_filter(store.clone(), outgoing_service.clone());
        info!(target: "interledger-node", "Settlement API listening on: {}", settlement_api_bind_address);
        spawn(warp::serve(settlement_api).bind(settlement_api_bind_address));

        // Exchange Rate Polling
        if let Some(provider) = exchange_rate_provider {
            let exchange_rate_fetcher = ExchangeRateFetcher::new(
                provider,
                exchange_rate_poll_failure_tolerance,
                store.clone(),
            );
            exchange_rate_fetcher
                .spawn_interval(Duration::from_millis(exchange_rate_poll_interval));
        } else {
            debug!(target: "interledger-node", "Not using exchange rate provider. Rates must be set via the HTTP API");
        }

        Ok(())
    }
}
