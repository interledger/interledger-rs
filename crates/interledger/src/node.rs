use bytes::Bytes;
use futures::Future;
use hex::FromHex;
use interledger_api::{NodeApi, NodeStore, NodeAccount};
use interledger_btp::{connect_client, create_server, BtpStore};
use interledger_ccp::CcpRouteManager;
use interledger_http::HttpClientService;
use interledger_ildcp::{IldcpAccount, IldcpService};
use interledger_packet::{ErrorCode, RejectBuilder};
use interledger_router::Router;
use interledger_service::{
    outgoing_service_fn, Account as AccountTrait, AccountStore, OutgoingRequest,
};
use interledger_service_util::{
    BalanceService, ExchangeRateService, ExpiryShortenerService, MaxPacketAmountService,
    RateLimitService, ValidatorService,
};
use interledger_store_redis::{
    connect as connect_redis_store, Account, ConnectionInfo, IntoConnectionInfo, RedisStore,
};
use interledger_stream::StreamReceiverService;
use ring::{digest, hmac};
use serde::{de::Error as DeserializeError, Deserialize, Deserializer};
use std::{net::SocketAddr, str};
use tokio::{self, net::TcpListener};
use tower_web::ServiceBuilder;
use url::Url;

static REDIS_SECRET_GENERATION_STRING: &str = "ilp_redis_secret";

///{} An all-in-one Interledger node that includes sender and receiver functionality,
/// a connector, and a management API. The node uses Redis for persistence.
#[derive(Deserialize, Clone)]
pub struct InterledgerNode {
    pub ilp_address: String,
    pub asset_code: String,
    pub asset_scale: u8,
    #[serde(deserialize_with = "deserialize_32_bytes_hex")]
    pub server_secret: [u8; 32],
    pub admin_auth_token: String,
    #[serde(deserialize_with = "deserialize_redis_connection")]
    pub redis_connection: ConnectionInfo,
    pub http_address: SocketAddr,
    // TODO should btp_address be optional?
    pub btp_address: SocketAddr,
}

fn deserialize_32_bytes_hex<'de, D>(deserializer: D) -> Result<[u8; 32], D::Error>
where
    D: Deserializer<'de>,
{
    <[u8; 32]>::from_hex(String::deserialize(deserializer)?)
        .map_err(|err| DeserializeError::custom(format!("Invalid hex value: {:?}", err)))
}

fn deserialize_redis_connection<'de, D>(deserializer: D) -> Result<ConnectionInfo, D::Error>
where
    D: Deserializer<'de>,
{
    Url::parse(String::deserialize(deserializer)?.as_str())
        .map_err(|err| DeserializeError::custom(format!("Invalid URL: {:?}", err)))?
        .into_connection_info()
        .map_err(|err| {
            DeserializeError::custom(format!(
                "Error converting into Redis connection info: {:?}",
                err
            ))
        })
}

impl InterledgerNode {
    // TODO when a BTP connection is made, insert a outgoing HTTP entry into the Store to tell other
    // connector instances to forward packets for that account to us
    pub fn serve(&self) -> impl Future<Item = (), Error = ()> {
        debug!("Starting Interledger node with ILP address: {}", self.ilp_address);
        let redis_secret = generate_redis_secret(&self.server_secret);
        let server_secret = Bytes::from(&self.server_secret[..]);
        let btp_address = self.btp_address.clone();
        let http_address = self.http_address.clone();
        let ilp_address = self.ilp_address.clone();
        let asset_code = self.asset_code.clone();
        let admin_auth_token = self.admin_auth_token.clone();
        let asset_scale = self.asset_scale;

        connect_redis_store(self.redis_connection.clone(), redis_secret)
        .map_err(|err| error!("Error connecting to Redis: {:?}", err))
        .and_then(move |store| {
            get_or_insert_default_account(store.clone(), ilp_address, asset_code, asset_scale, admin_auth_token)
                .join(store.clone().get_btp_outgoing_accounts())
                .map_err(|_| error!("Error getting accounts"))
                .and_then(move |(default_account, btp_accounts)| {
                    let ilp_address = Bytes::from(default_account.client_address());
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
                                triggered_by: &ilp_address[..],
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
                                    let ilp_address = Bytes::from(default_account.client_address());
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
                                    let incoming_service = CcpRouteManager::new(
                                        default_account.clone(),
                                        store.clone(),
                                        outgoing_service,
                                        incoming_service,
                                    );

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
                                    let api = NodeApi::new(
                                        server_secret,
                                        store.clone(),
                                        incoming_service.clone(),
                                    );
                                    let listener = TcpListener::bind(&http_address)
                                        .expect("Unable to bind to HTTP address");
                                    info!("Interledger node listening on: {}", http_address);
                                    let server = ServiceBuilder::new()
                                        .resource(api)
                                        .serve(listener.incoming());
                                    tokio::spawn(server);
                                    Ok(())
                                },
                            )
                        },
                    )
                })
        })
    }

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

fn get_or_insert_default_account(
    store: RedisStore,
    ilp_address: String,
    asset_code: String,
    asset_scale: u8,
    admin_auth_token: String,
) -> impl Future<Item = Account, Error = ()> {
    let account_details = AccountDetails {
        ilp_address,
        asset_code,
        asset_scale,
        btp_incoming_token: None,
        btp_uri: None,
        http_endpoint: None,
        http_incoming_token: Some(admin_auth_token),
        http_outgoing_token: None,
        max_packet_amount: u64::max_value(),
        // TODO there shouldn't be any min balance
        min_balance: i64::min_value(),
        is_admin: true,
        settle_threshold: None,
        settle_to: None,
        send_routes: false,
        receive_routes: false,
        routing_relation: None,
        round_trip_time: None,
        packets_per_minute_limit: None,
        amount_per_minute_limit: None,
    };
    store
        .clone()
        .get_accounts(vec![0])
        .and_then(|mut accounts| {
            let account = accounts.pop().expect("Get accounts returned empty vec");
            if account.is_admin() {
                Ok(account)
            } else {
                // TODO make it so this can't happen
                error!("Another account was inserted before the admin account!");
                Err(())
            }
        })
        .or_else(move |_| {
            // TODO make sure the error comes from the fact that the account doesn't exist and isn't a different type of error
            debug!("Creating default account");
            store.insert_account(account_details)
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
