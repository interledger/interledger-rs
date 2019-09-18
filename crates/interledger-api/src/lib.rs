use crate::client::Client;
use bytes::{Buf, Bytes, BytesMut};
use futures::{
    future::{err, ok, result, Either},
    sync::mpsc::UnboundedSender,
    Future, Stream,
};
use interledger_http::{HttpAccount, HttpStore};
use interledger_ildcp::IldcpAccount;
use interledger_packet::{Address, Prepare};
use interledger_router::RouterStore;
use interledger_service::{Account, AuthToken, IncomingRequest, IncomingService, Username};
use interledger_service_util::{BalanceStore, ExchangeRateStore};
use interledger_settlement::{SettlementAccount, SettlementStore};
use interledger_spsp::{pay, SpspResponder};
use log::{debug, error, trace};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::{
    collections::HashMap,
    convert::TryFrom,
    error::Error as StdError,
    fmt::{self, Display},
    iter::FromIterator,
    net::SocketAddr,
    str::{self, FromStr},
    time::Duration,
};
use url::Url;
use warp::{self, Filter};

pub(crate) mod client;

const MAX_RETRIES: usize = 10;
// TODO use the exact max size of a packet
const MAX_PACKET_SIZE: u64 = 40000;

pub trait NodeStore: Clone + Send + Sync + 'static {
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

    // TODO this should take an AccountId instead of a String
    fn add_payment_notification_subscription(
        &self,
        account_id: String,
        sender: UnboundedSender<String>,
    );
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
#[derive(Debug, Clone, Deserialize)]
pub struct AccountDetails {
    pub ilp_address: Address,
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

#[derive(Deserialize, Debug)]
struct SpspPayRequest {
    receiver: String,
    source_amount: u64,
}

pub struct NodeApi<S, I> {
    store: S,
    admin_api_token: String,
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
    A: Account + HttpAccount + IldcpAccount + SettlementAccount + Serialize + Send + Sync + 'static,
{
    pub fn new(
        server_secret: Bytes,
        admin_api_token: String,
        store: S,
        incoming_handler: I,
    ) -> Self {
        NodeApi {
            store,
            admin_api_token,
            default_spsp_account: None,
            incoming_handler,
            server_secret,
        }
    }

    pub fn default_spsp_account(&mut self, username: Username) -> &mut Self {
        self.default_spsp_account = Some(username);
        self
    }

    pub fn into_warp_filter(self) -> warp::filters::BoxedFilter<(impl warp::Reply,)> {
        node_api(
            self.server_secret,
            self.admin_api_token,
            self.default_spsp_account,
            self.incoming_handler,
            self.store,
        )
    }

    pub fn bind(self, addr: SocketAddr) -> impl Future<Item = (), Error = ()> {
        warp::serve(self.into_warp_filter()).bind(addr)
    }
}

#[allow(clippy::enum_variant_names)]
#[derive(Clone, Copy, Debug)]
enum Error {
    AccountNotFound,
    BadRequest,
    InternalServerError,
    Unauthorized,
}

impl Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str(match self {
            Error::AccountNotFound => "Account not found",
            Error::BadRequest => "Bad request",
            Error::InternalServerError => "Internal server error",
            Error::Unauthorized => "Unauthorized",
        })
    }
}

impl StdError for Error {}

fn node_api<I, S, A>(
    server_secret: Bytes,
    admin_api_token: String,
    default_spsp_account: Option<Username>,
    incoming_handler: I,
    store: S,
) -> warp::filters::BoxedFilter<(impl warp::Reply,)>
where
    I: IncomingService<A> + Clone + Send + Sync + 'static,
    S: NodeStore<Account = A>
        + HttpStore<Account = A>
        + BalanceStore<Account = A>
        + ExchangeRateStore
        + RouterStore,
    A: Account + IldcpAccount + HttpAccount + Serialize + 'static,
{
    // TODO can we make any of the Filters const or put them in lazy_static?

    // Helper filters
    let admin_auth_header = format!("Bearer {}", admin_api_token);
    let admin_only = warp::header::<String>("authorization")
        .and_then(move |authorization| {
            if authorization == admin_auth_header {
                Ok(())
            } else {
                Err(warp::reject::custom(Error::Unauthorized))
            }
        })
        .untuple_one();
    let with_store = warp::any().map(move || store.clone());
    let with_incoming_handler = warp::any().map(move || incoming_handler.clone());
    let accounts = warp::path("accounts");
    let accounts_index = accounts.and(warp::path::end());
    let account_username = accounts.and(warp::path::param2::<Username>());
    let account_username_to_id =
        account_username
            .and(with_store.clone())
            .and_then(|username: Username, store: S| {
                store
                    .get_account_id_from_username(&username)
                    .map_err(move |_| {
                        // TODO differentiate between server error and not found
                        error!("Error getting account id from username: {}", username);
                        warp::reject::custom(Error::AccountNotFound)
                    })
            });
    let valid_account_authorization = warp::header::<AuthToken>("authorization")
        .and(with_store.clone())
        .and_then(|auth: AuthToken, store: S| {
            store
                .get_account_from_http_auth(auth.username(), auth.password())
                .map_err(move |_| {
                    error!(
                        "Invalid authorization provided for user: {}",
                        auth.username()
                    );
                    warp::reject::custom(Error::Unauthorized)
                })
        });
    let authorized_account_from_path = account_username
        .and(valid_account_authorization.clone())
        .and_then(|path_username: Username, authorized_account: A| {
            // Check that the user is authorized for this route
            if &path_username == authorized_account.username() {
                Ok(authorized_account)
            } else {
                Err(warp::reject::custom(Error::Unauthorized))
            }
        });
    let admin_or_authorized_account = admin_only
        .clone()
        .and(account_username_to_id.clone())
        .clone()
        .or(authorized_account_from_path
            .clone()
            .map(|account: A| account.id()))
        .unify();

    // POST /accounts
    let http_client = Client::new(Duration::from_millis(5000), MAX_RETRIES);
    let post_accounts = warp::post2()
        .and(accounts_index)
        .and(admin_only.clone())
        .and(warp::body::json())
        .and(with_store.clone())
        .and_then(move |account_details: AccountDetails, store: S| {
            let settlement_engine_url = account_details.settlement_engine_url.clone();
            let http_client = http_client.clone();
            store.insert_account(account_details)
                .map_err(|_| {
                    warp::reject::custom(Error::InternalServerError)
                })
                .and_then(move |account: A| {
                    let http_client = http_client.clone();
                    // Register the account with the settlement engine
                    // if a settlement_engine_url was configured on the account
                    if let Some(se_url) = settlement_engine_url {
                        let id = account.id();
                        Either::A(result(Url::parse(&se_url))
                        .map_err(|_| {
                            // TODO include a more specific error message
                            warp::reject::custom(Error::BadRequest)
                        })
                        .and_then(move |se_url| {
                            let http_client = http_client.clone();
                            trace!(
                                "Sending account {} creation request to settlement engine: {:?}",
                                id,
                                se_url.clone()
                            );
                            http_client.create_engine_account(se_url, id)
                            .and_then(move |status_code| {
                                if status_code.is_success() {
                                    trace!("Account {} created on the SE", id);
                                } else {
                                    error!("Error creating account. Settlement engine responded with HTTP code: {}", status_code);
                                }
                                Ok(())
                            })
                            .map_err(|_| warp::reject::custom(Error::InternalServerError))
                            .and_then(move |_| {
                                Ok(account)
                            })
                        }))
                    } else {
                        Either::B(ok(account))
                    }
                })
                .and_then(|account: A| {
                    Ok(warp::reply::json(&account))
                })
        });

    // GET /accounts
    let get_accounts = warp::get2()
        .and(accounts_index)
        .and(admin_only.clone())
        .and(with_store.clone())
        .and_then(|store: S| {
            store
                .get_all_accounts()
                .map_err(|_| warp::reject::custom(Error::InternalServerError))
                .and_then(|accounts| Ok(warp::reply::json(&accounts)))
        });

    // PUT /accounts/:username
    let put_account = warp::put2()
        .and(account_username_to_id.clone())
        .and(warp::path::end())
        .and(admin_only.clone())
        .and(warp::body::json())
        .and(with_store.clone())
        .and_then(
            |id: A::AccountId, account_details: AccountDetails, store: S| {
                store
                    .update_account(id, account_details)
                    .map_err(move |_| warp::reject::custom(Error::InternalServerError))
                    .and_then(move |account| Ok(warp::reply::json(&account)))
            },
        );

    // GET /accounts/:username
    let get_account = warp::get2()
        .and(admin_or_authorized_account.clone())
        .and(warp::path::end())
        .and(with_store.clone())
        .and_then(|id: A::AccountId, store: S| {
            store
                .get_accounts(vec![id])
                .map_err(|_| warp::reject::not_found())
                .and_then(|accounts| Ok(warp::reply::json(&accounts[0])))
        });

    // GET /accounts/:username/balance
    let get_account_balance = warp::get2()
        .and(admin_or_authorized_account.clone())
        .and(warp::path("balance"))
        .and(warp::path::end())
        .and(with_store.clone())
        .and_then(|id: A::AccountId, store: S| {
            // TODO reduce the number of store calls it takes to get the balance
            store
                .get_accounts(vec![id])
                .map_err(|_| warp::reject::not_found())
                .and_then(move |mut accounts| {
                    store
                        .get_balance(accounts.pop().unwrap())
                        .map_err(move |_| {
                            error!("Error getting balance for account: {}", id);
                            warp::reject::custom(Error::InternalServerError)
                        })
                })
                .and_then(|balance: i64| {
                    Ok(warp::reply::json(&json!({
                        "balance": balance.to_string(),
                    })))
                })
        });

    // DELETE /accounts/:username
    let delete_account = warp::delete2()
        .and(admin_only.clone())
        .and(account_username_to_id.clone())
        .and(warp::path::end())
        .and(with_store.clone())
        .and_then(|id: A::AccountId, store: S| {
            store
                .delete_account(id)
                .map_err(move |_| {
                    error!("Error deleting account {}", id);
                    warp::reject::custom(Error::InternalServerError)
                })
                .and_then(|account| Ok(warp::reply::json(&account)))
        });

    // PUT /accounts/:username/settings
    let put_account_settings = warp::put2()
        .and(admin_or_authorized_account.clone())
        .and(warp::path("settings"))
        .and(warp::path::end())
        .and(warp::body::json())
        .and(with_store.clone())
        .and_then(|id: A::AccountId, settings: AccountSettings, store: S| {
            store
                .modify_account_settings(id, settings)
                .map_err(move |_| {
                    error!("Error updating account settings {}", id);
                    warp::reject::custom(Error::InternalServerError)
                })
                .and_then(|settings| Ok(warp::reply::json(&settings)))
        });

    // (Websocket) /accounts/:username/payments/incoming
    let incoming_payment_notifications = warp::ws2()
        .and(admin_or_authorized_account.clone())
        .and(warp::path("payments"))
        .and(warp::path("incoming"))
        .and(warp::path::end())
        .and(with_store.clone())
        .map(|ws: warp::ws::Ws2, id: A::AccountId, store: S| {
            ws.on_upgrade(move |ws: warp::ws::WebSocket| {
                let (tx, rx) = futures::sync::mpsc::unbounded::<String>();
                store.add_payment_notification_subscription(id.to_string(), tx);
                rx.map_err(|_| -> warp::Error { unreachable!("unbounded rx never errors") })
                    .map(warp::ws::Message::text)
                    .forward(ws)
                    .map(|_| ())
                    .map_err(|err| error!("Error forwarding notifications to websocket: {:?}", err))
            })
        });

    // POST /ilp
    // TODO should this be under /accounts?
    let post_ilp = warp::post2()
        .and(warp::path("ilp"))
        .and(warp::path::end())
        .and(valid_account_authorization.clone())
        .and(warp::body::content_length_limit(MAX_PACKET_SIZE))
        .and(warp::body::concat())
        .and(with_incoming_handler.clone())
        .and_then(|account: A, body: warp::body::FullBody, mut incoming: I| {
            // TODO don't copy ILP packet
            let buffer = BytesMut::from(body.bytes());
            if let Ok(prepare) = Prepare::try_from(buffer) {
                Either::A(
                    incoming
                        .handle_request(IncomingRequest {
                            from: account,
                            prepare,
                        })
                        .then(|result| {
                            let bytes: BytesMut = match result {
                                Ok(fulfill) => fulfill.into(),
                                Err(reject) => reject.into(),
                            };
                            Ok(warp::http::Response::builder()
                                .header("Content-Type", "application/octet-stream")
                                .status(200)
                                .body(bytes.freeze())
                                .unwrap())
                        }),
                )
            } else {
                error!("Body was not a valid Prepare packet");
                Either::B(err(warp::reject::custom(Error::BadRequest)))
            }
        });

    // POST /accounts/:username/payments
    let post_payments = warp::post2()
        .and(authorized_account_from_path.clone())
        .and(warp::path("payments"))
        .and(warp::path::end())
        .and(warp::body::json())
        .and(with_incoming_handler.clone())
        .and_then(
            |account: A, pay_request: SpspPayRequest, incoming_handler: I| {
                pay(
                    incoming_handler,
                    account,
                    &pay_request.receiver,
                    pay_request.source_amount,
                )
                .and_then(|delivered_amount| {
                    debug!(
                        "Sent SPSP payment and delivered: {} of the receiver's units",
                        delivered_amount
                    );
                    Ok(warp::reply::json(&json!({
                        "delivered_amount": delivered_amount
                    })))
                })
                .map_err(|err| {
                    error!("Error sending SPSP payment: {:?}", err);
                    // TODO give a different error message depending on what type of error it is
                    warp::reject::custom(Error::InternalServerError)
                })
            },
        );

    // GET /accounts/:username/spsp
    let server_secret_clone = server_secret.clone();
    let get_spsp = warp::get2()
        .and(account_username_to_id.clone())
        .and(warp::path("spsp"))
        .and(warp::path::end())
        .and(with_store.clone())
        .and_then(move |id: A::AccountId, store: S| {
            let server_secret_clone = server_secret_clone.clone();
            store
                .get_accounts(vec![id])
                .map_err(|_| warp::reject::custom(Error::InternalServerError))
                .and_then(move |accounts| {
                    // TODO return the response without instantiating an SpspResponder (use a simple fn)
                    Ok(SpspResponder::new(
                        accounts[0].client_address().clone(),
                        server_secret_clone.clone(),
                    )
                    .generate_http_response())
                })
        });

    // GET /.well-known/pay
    let server_secret_clone2 = server_secret.clone();
    let get_spsp_well_known = warp::get2()
        .and(warp::path(".well-known"))
        .and(warp::path("pay"))
        .and(warp::path::end())
        .and(with_store.clone())
        .and_then(move |store: S| {
            // TODO don't clone this
            if let Some(username) = default_spsp_account.clone() {
                let server_secret_clone2 = server_secret_clone2.clone();
                Either::A(
                    store
                        .get_account_id_from_username(&username)
                        .map_err(move |_| {
                            error!("Account not found: {}", username);
                            warp::reject::not_found()
                        })
                        .and_then(move |id| {
                            // TODO this shouldn't take multiple store calls
                            store
                                .get_accounts(vec![id])
                                .map_err(|_| warp::reject::custom(Error::InternalServerError))
                                .map(|mut accounts| accounts.pop().unwrap())
                        })
                        .and_then(move |account| {
                            // TODO return the response without instantiating an SpspResponder (use a simple fn)
                            Ok(SpspResponder::new(
                                account.client_address().clone(),
                                server_secret_clone2.clone(),
                            )
                            .generate_http_response())
                        }),
                )
            } else {
                Either::B(err(warp::reject::not_found()))
            }
        });

    // GET /
    let get_root = warp::get2().and(warp::path::end()).map(|| {
        // TODO add more to this response
        warp::reply::json(&json!({
            "status": "Ready".to_string(),
        }))
    });

    // PUT /rates
    let put_rates = warp::put2()
        .and(admin_only.clone())
        .and(warp::path("rates"))
        .and(warp::path::end())
        .and(warp::body::json())
        .and(with_store.clone())
        .and_then(|rates: HashMap<String, f64>, store: S| {
            if store.set_exchange_rates(rates.clone()).is_ok() {
                Ok(warp::reply::json(&rates))
            } else {
                error!("Error setting exchange rates");
                Err(warp::reject::custom(Error::InternalServerError))
            }
        });

    // GET /rates
    let get_rates = warp::get2()
        .and(warp::path("rates"))
        .and(warp::path::end())
        .and(with_store.clone())
        .and_then(|store: S| {
            if let Ok(rates) = store.get_all_exchange_rates() {
                Ok(warp::reply::json(&rates))
            } else {
                error!("Error getting exchange rates");
                Err(warp::reject::custom(Error::InternalServerError))
            }
        });

    // GET /routes
    let get_routes = warp::get2()
        .and(warp::path("routes"))
        .and(warp::path::end())
        .and(with_store.clone())
        .map(|store: S| {
            // Convert addresses from bytes to utf8 strings
            let routes: HashMap<String, String> =
                HashMap::from_iter(store.routing_table().into_iter().filter_map(
                    |(address, account)| {
                        if let Ok(address) = str::from_utf8(address.as_ref()) {
                            Some((address.to_string(), account.to_string()))
                        } else {
                            None
                        }
                    },
                ));
            warp::reply::json(&routes)
        });

    // PUT /routes/static
    let put_static_routes = warp::put2()
        .and(warp::path("routes"))
        .and(warp::path("static"))
        .and(warp::path::end())
        .and(warp::body::json())
        .and(with_store.clone())
        .and_then(|routes: HashMap<String, String>, store: S| {
            let mut parsed = HashMap::with_capacity(routes.len());
            for (prefix, id) in routes.into_iter() {
                if let Ok(id) = A::AccountId::from_str(id.as_str()) {
                    parsed.insert(prefix, id);
                } else {
                    error!("Invalid Account ID: {}", id);
                    return Either::B(err(warp::reject::custom(Error::BadRequest)));
                }
            }
            Either::A(
                store
                    .set_static_routes(parsed.clone())
                    .map_err(|_| {
                        error!("Error setting static routes");
                        warp::reject::custom(Error::InternalServerError)
                    })
                    .map(move |_| warp::reply::json(&parsed)),
            )
        });

    // PUT /routes/static/:prefix
    let put_static_route = warp::put2()
        .and(warp::path("routes"))
        .and(warp::path("static"))
        .and(warp::path::param2::<String>())
        .and(warp::path::end())
        .and(warp::body::concat())
        .and(with_store.clone())
        .and_then(|prefix: String, body: warp::body::FullBody, store: S| {
            if let Ok(string) = str::from_utf8(body.bytes()) {
                if let Ok(id) = A::AccountId::from_str(string) {
                    return Either::A(
                        store
                            .set_static_route(prefix, id)
                            .map_err(|_| {
                                error!("Error setting static route");
                                warp::reject::custom(Error::InternalServerError)
                            })
                            .map(move |_| id.to_string()),
                    );
                }
            }
            error!("Body was not a valid Account ID");
            Either::B(err(warp::reject::custom(Error::BadRequest)))
        });

    let api = post_ilp
        .or(get_spsp)
        .or(get_spsp_well_known)
        .or(post_accounts)
        .or(get_accounts)
        .or(put_account)
        .or(get_account)
        .or(get_account_balance)
        .or(delete_account)
        .or(put_account_settings)
        .or(incoming_payment_notifications)
        .or(post_payments)
        .or(get_root)
        .or(put_rates)
        .or(get_rates)
        .or(get_routes)
        .or(put_static_routes)
        .or(put_static_route);

    api.with(warp::log("interledger-api"))
        .recover(|err: warp::Rejection| {
            if let Some(&err) = err.find_cause::<Error>() {
                let code = match err {
                    Error::AccountNotFound => warp::http::StatusCode::NOT_FOUND,
                    Error::BadRequest => warp::http::StatusCode::BAD_REQUEST,
                    Error::InternalServerError => warp::http::StatusCode::INTERNAL_SERVER_ERROR,
                    Error::Unauthorized => warp::http::StatusCode::UNAUTHORIZED,
                };
                Ok(warp::reply::with_status(warp::reply(), code))
            } else {
                Err(err)
            }
        })
        .boxed()
}
