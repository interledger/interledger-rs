use bytes::Bytes;
use futures::Future;
use hex::FromHex;
use interledger_api::{NodeApi, NodeStore};
use interledger_btp::{connect_client, create_server, BtpStore};
use interledger_ccp::CcpRouteManagerBuilder;
use interledger_http::HttpClientService;
use interledger_ildcp::IldcpService;
use interledger_packet::{ErrorCode, RejectBuilder};
use interledger_router::Router;
use interledger_service::{outgoing_service_fn, Account as AccountTrait, OutgoingRequest};
use interledger_service_util::{
    BalanceService, ExchangeRateService, ExpiryShortenerService, MaxPacketAmountService,
    RateLimitService, ValidatorService,
};
use interledger_store_redis::{
    connect as connect_redis_store, Account, ConnectionInfo, IntoConnectionInfo,
};
use interledger_stream::StreamReceiverService;
use ring::{digest, hmac};
use serde::{de::Error as DeserializeError, Deserialize, Deserializer};
use std::{net::SocketAddr, str};
use tokio::{self, net::TcpListener};
use url::Url;

static REDIS_SECRET_GENERATION_STRING: &str = "ilp_redis_secret";
static DEFAULT_REDIS_URL: &str = "redis://127.0.0.1:6379";

fn default_http_address() -> SocketAddr {
    SocketAddr::from(([127, 0, 0, 1], 7770))
}
fn default_btp_address() -> SocketAddr {
    SocketAddr::from(([127, 0, 0, 1], 7768))
}
fn default_redis_uri() -> ConnectionInfo {
    DEFAULT_REDIS_URL.into_connection_info().unwrap()
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
    // Rename this one because the env vars are prefixed with "ILP_"
    #[serde(alias = "address")]
    pub ilp_address: String,
    /// Root secret used to derive encryption keys
    #[serde(deserialize_with = "deserialize_32_bytes_hex")]
    pub server_secret: [u8; 32],
    /// HTTP Authorization token for the node admin (sent as a Bearer token)
    pub admin_auth_token: String,
    /// Redis URI (for example, "redis://127.0.0.1:6379" or "unix:/tmp/redis.sock")
    #[serde(deserialize_with = "deserialize_redis_connection", default = "default_redis_uri")]
    pub redis_connection: ConnectionInfo,
    /// IP address and port to listen for HTTP connections on
    /// This is used for both the API and ILP over HTTP packets
    #[serde(default = "default_http_address")]
    pub http_address: SocketAddr,
    /// IP address and port to listen for BTP connections on
    #[serde(default = "default_btp_address")]
    pub btp_address: SocketAddr,
    /// When SPSP payments are sent to the root domain, the payment pointer is resolved
    /// to <domain>/.well-known/pay. This value determines which account those payments
    /// will be sent to.
    pub default_spsp_account: Option<u64>,
}

impl InterledgerNode {
    /// Returns a future that runs the Interledger Node
    // TODO when a BTP connection is made, insert a outgoing HTTP entry into the Store to tell other
    // connector instances to forward packets for that account to us
    pub fn serve(&self) -> impl Future<Item = (), Error = ()> {
        debug!(
            "Starting Interledger node with ILP address: {}",
            str::from_utf8(self.ilp_address.as_ref()).unwrap_or("<not utf8>")
        );
        let redis_secret = generate_redis_secret(&self.server_secret);
        let server_secret = Bytes::from(&self.server_secret[..]);
        let btp_address = self.btp_address.clone();
        let http_address = self.http_address.clone();
        let ilp_address = Bytes::from(self.ilp_address.as_str());
        let ilp_address_clone = ilp_address.clone();
        let admin_auth_token = self.admin_auth_token.clone();
        let default_spsp_account = self.default_spsp_account.clone();
        let redis_addr = self.redis_connection.addr.clone();

        connect_redis_store(self.redis_connection.clone(), redis_secret)
        .map_err(move |err| error!("Error connecting to Redis: {:?} {:?}", redis_addr, err))
        .and_then(move |store| {
                store.clone().get_btp_outgoing_accounts()
                .map_err(|_| error!("Error getting accounts"))
                .and_then(move |btp_accounts| {
                    let outgoing_service =
                        outgoing_service_fn(move |request: OutgoingRequest<Account>| {
                            error!("No route found for outgoing account {}", request.to.id());
                            trace!("Rejecting request to account {}, prepare packet: {:?}", request.to.id(), request.prepare);
                            Err(RejectBuilder {
                                code: ErrorCode::F02_UNREACHABLE,
                                message: &format!(
                                    "No outgoing route for account: {} (ILP address of the Prepare packet: {})",
                                    request.to.id(),
                                    str::from_utf8(request.prepare.destination())
                                        .unwrap_or("<not utf8>")
                                )
                                .as_bytes(),
                                triggered_by: ilp_address_clone.as_ref(),
                                data: &[],
                            }
                            .build())
                        });

                    // Connect to all of the accounts that have outgoing btp_uris configured
                    // but don't fail if we are unable to connect
                    // TODO try reconnecting to those accounts later
                    connect_client(btp_accounts, false, outgoing_service).and_then(
                        move |btp_client_service| {
                            create_server(btp_address, store.clone(), btp_client_service.clone()).and_then(
                                move |btp_server_service| {
                                    // The BTP service is both an Incoming and Outgoing one so we pass it first as the Outgoing
                                    // service to others like the router and then call handle_incoming on it to set up the incoming handler
                                    let outgoing_service = btp_server_service.clone();
                                    let outgoing_service =
                                        ValidatorService::outgoing(outgoing_service);
                                    let outgoing_service = HttpClientService::new(store.clone(), outgoing_service);

                                    // Note: the expiry shortener must come after the Validator so that the expiry duration
                                    // is shortened before we check whether there is enough time left
                                    let outgoing_service =
                                        ExpiryShortenerService::new(outgoing_service);
                                    let outgoing_service = StreamReceiverService::new(
                                        server_secret.clone(),
                                        outgoing_service,
                                    );
                                    let outgoing_service = BalanceService::new(
                                        ilp_address.clone(),
                                        store.clone(),
                                        outgoing_service,
                                    );
                                    let outgoing_service = ExchangeRateService::new(
                                        ilp_address.clone(),
                                        store.clone(),
                                        outgoing_service,
                                    );

                                    // Set up the Router and Routing Manager
                                    let incoming_service =
                                        Router::new(store.clone(), outgoing_service.clone());
                                    let incoming_service = CcpRouteManagerBuilder::new(
                                        store.clone(),
                                        outgoing_service,
                                        incoming_service,
                                    ).ilp_address(ilp_address.clone()).to_service();

                                    let incoming_service = IldcpService::new(incoming_service);
                                    let incoming_service =
                                        MaxPacketAmountService::new(incoming_service);
                                    let incoming_service =
                                        ValidatorService::incoming(incoming_service);
                                    let incoming_service = RateLimitService::new(
                                        ilp_address.clone(),
                                        store.clone(),
                                        incoming_service,
                                    );

                                    // Handle incoming packets sent via BTP
                                    btp_server_service.handle_incoming(incoming_service.clone());
                                    btp_client_service.handle_incoming(incoming_service.clone());

                                    // TODO should this run the node api on a different port so it's easier to separate public/private?
                                    // Note the API also includes receiving ILP packets sent via HTTP
                                    let mut api = NodeApi::new(
                                        server_secret,
                                        admin_auth_token,
                                        store.clone(),
                                        incoming_service.clone(),
                                    );
                                    if let Some(account_id) = default_spsp_account {
                                        api.default_spsp_account(format!("{}", account_id));
                                    }
                                    let listener = TcpListener::bind(&http_address)
                                        .expect("Unable to bind to HTTP address");
                                    info!("Interledger node listening on: {}", http_address);
                                    tokio::spawn(api.serve(listener.incoming()));
                                    Ok(())
                                },
                            )
                        },
                    )
                })
        })
    }

    /// Run the node on the default Tokio runtime
    pub fn run(&self) {
        tokio::run(self.serve());
    }

    pub fn insert_account(&self, account: AccountDetails) -> impl Future<Item = (), Error = ()> {
        insert_account_redis(self.redis_connection.clone(), &self.server_secret, account)
    }
}

#[doc(hidden)]
pub use interledger_api::AccountDetails;
#[doc(hidden)]
pub fn insert_account_redis<R>(
    redis_uri: R,
    server_secret: &[u8; 32],
    account: AccountDetails,
) -> impl Future<Item = (), Error = ()>
where
    R: IntoConnectionInfo,
{
    let redis_secret = generate_redis_secret(server_secret);
    connect_redis_store(redis_uri, redis_secret)
        .map_err(|err| error!("Error connecting to Redis: {:?}", err))
        .and_then(move |store| {
            store
                .insert_account(account)
                .map_err(|_| error!("Unable to create account"))
                .and_then(|account| {
                    debug!("Created account: {}", account.id());
                    Ok(())
                })
        })
}

fn generate_redis_secret(server_secret: &[u8; 32]) -> [u8; 32] {
    let mut redis_secret: [u8; 32] = [0; 32];
    let sig = hmac::sign(
        &hmac::SigningKey::new(&digest::SHA256, server_secret),
        REDIS_SECRET_GENERATION_STRING.as_bytes(),
    );
    redis_secret.copy_from_slice(sig.as_ref());
    redis_secret
}
