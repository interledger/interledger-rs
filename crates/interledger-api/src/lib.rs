#![recursion_limit = "128"]
#[macro_use]
extern crate tower_web;

use bytes::Bytes;
use futures::{
    future::{ok, result, Either},
    sync::mpsc::UnboundedSender,
    Future, Sink, Stream,
};
use interledger_http::{HttpAccount, HttpStore};
use interledger_ildcp::IldcpAccount;
use interledger_packet::Address;
use interledger_router::RouterStore;
use interledger_service::{Account as AccountTrait, IncomingService, Username};
use interledger_service_util::{BalanceStore, ExchangeRateStore};
use interledger_settlement::{SettlementAccount, SettlementStore};
use lazy_static::lazy_static;
use log::{debug, error, info, warn};
use parking_lot::Mutex;
use regex::Regex;
use serde::Serialize;
use std::io::{Error as IoError, ErrorKind};
use std::net::SocketAddr;
use std::str::{self, FromStr};
use std::sync::Arc;
use tokio::net::tcp::Incoming;
use tokio::spawn;
use tokio_tungstenite::accept_hdr_async;
use tower_web::{net::ConnectionStream, Extract, Response, ServiceBuilder};
use tungstenite::{handshake::server::Request, Error as WebSocketError, Message};

lazy_static! {
    static ref USERNAME_FROM_PATH: Regex =
        Regex::new(r"accounts/(?P<user>\w+)/incoming_payments$").unwrap();
}

mod routes;
use self::routes::*;

pub(crate) mod client;

pub(crate) const BEARER_TOKEN_START: usize = 7;

pub trait NodeStore: Clone + Send + Sync + 'static {
    type Account: AccountTrait;

    fn insert_account(
        &self,
        account: AccountDetails,
    ) -> Box<dyn Future<Item = Self::Account, Error = ()> + Send>;

    fn delete_account(
        &self,
        id: <Self::Account as AccountTrait>::AccountId,
    ) -> Box<dyn Future<Item = Self::Account, Error = ()> + Send>;

    fn update_account(
        &self,
        id: <Self::Account as AccountTrait>::AccountId,
        account: AccountDetails,
    ) -> Box<dyn Future<Item = Self::Account, Error = ()> + Send>;

    fn modify_account_settings(
        &self,
        id: <Self::Account as AccountTrait>::AccountId,
        settings: AccountSettings,
    ) -> Box<dyn Future<Item = Self::Account, Error = ()> + Send>;

    // TODO limit the number of results and page through them
    fn get_all_accounts(&self) -> Box<dyn Future<Item = Vec<Self::Account>, Error = ()> + Send>;

    fn set_static_routes<R>(&self, routes: R) -> Box<dyn Future<Item = (), Error = ()> + Send>
    where
        R: IntoIterator<Item = (String, <Self::Account as AccountTrait>::AccountId)>;

    fn set_static_route(
        &self,
        prefix: String,
        account_id: <Self::Account as AccountTrait>::AccountId,
    ) -> Box<dyn Future<Item = (), Error = ()> + Send>;

    fn add_payment_notification_subscription(&self, id: String, sender: UnboundedSender<String>);
}

/// AccountSettings is a subset of the user parameters defined in
/// AccountDetails. Its purpose is to allow a user to modify certain of their
/// parameters which they may want to re-configure in the future, such as their
/// tokens (which act as passwords), their settlement frequency preferences, or
/// their HTTP/BTP endpoints, since they may change their network configuration.
#[derive(Debug, Extract, Response, Clone, Default)]
pub struct AccountSettings {
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
#[derive(Debug, Extract, Response, Clone)]
pub struct AccountDetails {
    pub ilp_address: Address,
    pub username: Username,
    pub asset_code: String,
    pub asset_scale: u8,
    #[serde(default = "u64::max_value")]
    pub max_packet_amount: u64,
    pub min_balance: Option<i64>,
    pub http_endpoint: Option<String>,
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

pub struct NodeApi<S, I> {
    store: S,
    admin_api_token: String,
    notifications_address: SocketAddr,
    default_spsp_account: Option<Username>,
    incoming_handler: I,
    server_secret: Bytes,
}

impl<S, I, A> NodeApi<S, I>
where
    S: NodeStore<Account = A>
        + HttpStore<Account = A>
        + BalanceStore<Account = A>
        + SettlementStore<Account = A>
        + RouterStore
        + ExchangeRateStore,
    I: IncomingService<A> + Clone + Send + Sync + 'static,
    A: AccountTrait
        + HttpAccount
        + IldcpAccount
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
        notifications_address: SocketAddr,
        incoming_handler: I,
    ) -> Self {
        NodeApi {
            store,
            admin_api_token,
            notifications_address,
            default_spsp_account: None,
            incoming_handler,
            server_secret,
        }
    }

    pub fn default_spsp_account(&mut self, username: Username) -> &mut Self {
        self.default_spsp_account = Some(username);
        self
    }

    // TODO: implement generically rather than over Incoming
    pub fn serve_notifications(&self, incoming: Incoming) -> impl Future<Item = (), Error = ()> {
        let store = self.store.clone();
        incoming
            .map_err(|e| error!("Failed to handle incoming connection: {}", e))
            .for_each(move |stream| {
                let store_clone = store.clone();

                // Used to make HTTP header details accessible to the websocket handler.
                // This is necessary because while the method for accepting websockets is async,
                // the callback used to process headers isn't.
                let details = Arc::new(Mutex::new(None));
                let details_clone = details.clone();

                // Processes a stream of incoming websocket connections while stashing relevant request details
                accept_hdr_async(stream, move |req: &Request| {
                    details_clone.lock().replace((
                        req.path.clone(),
                        req.headers.find_first("Authorization").and_then(|bytes| Some(bytes.to_vec())),
                    ));
                    Ok(None)
                })
                .map_err(|e| error!("Failed to accept websocket connection: {}", e))
                .and_then(move |ws_stream| {
                    let (tx, rx) = futures::sync::mpsc::unbounded::<String>();
                    let lock = details.lock();
                    let (ref path, ref _authorization) = lock.as_ref()
                        .expect("Details should have been parsed from the headers");
                    info!("Established incoming websocket connection to the resource at: {}", path);
                    match USERNAME_FROM_PATH.captures(&path) {
                        None => {
                            error!("Failed to identify valid request path");
                            Either::B(ok(()))
                        }
                        Some(captures) => match captures.name("user") {
                            None => {
                                error!("Failed to locate username in path");
                                Either::B(ok(()))
                            }
                            Some(username) => Either::A(
                                // TODO Validate authorization first
                                result(
                                    Username::from_str(username.as_str())
                                    .map_err(|e| error!("Failed to parse username: {}", e))
                                )
                                .and_then(move |username| {
                                    let username_clone = username.clone();
                                    store_clone.get_account_id_from_username(&username)
                                    .map_err(move |_| error!("Failed to find account with username: {}", username_clone))
                                    .and_then(move |_account_id| {
                                        // TODO: use account_id rather than username
                                        store_clone.add_payment_notification_subscription(username.to_string(), tx);
                                        spawn(
                                            ws_stream
                                                .send_all(rx.map(Message::Text).map_err(|_| {
                                                    warn!("Websocket connection aborted");
                                                    WebSocketError::from(IoError::from(
                                                        ErrorKind::ConnectionAborted,
                                                    ))
                                                }))
                                                .then(move |_| {
                                                    info!("Finished forwarding subscriber to account: {}", username);
                                                    Ok(())
                                                }),
                                        );
                                        Ok(())
                                    })
                                }),
                            )
                        }
                    }
                })
            })
    }

    pub fn serve<T>(&self, incoming: T) -> impl Future<Item = (), Error = ()>
    where
        T: ConnectionStream,
        T::Item: Send + 'static,
    {
        ServiceBuilder::new()
            .resource(IlpApi::new(
                self.store.clone(),
                self.incoming_handler.clone(),
            ))
            .resource({
                let mut spsp = SpspApi::new(
                    self.server_secret.clone(),
                    self.store.clone(),
                    self.incoming_handler.clone(),
                );
                if let Some(username) = &self.default_spsp_account {
                    spsp.default_spsp_account(username.clone());
                }
                spsp
            })
            .resource(AccountsApi::new(
                self.admin_api_token.clone(),
                self.notifications_address.clone(),
                self.store.clone(),
            ))
            .resource(SettingsApi::new(
                self.admin_api_token.clone(),
                self.store.clone(),
            ))
            .serve(incoming)
    }
}
