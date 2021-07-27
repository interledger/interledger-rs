use crate::{number_or_string, AccountDetails, AccountSettings, NodeStore};
use bytes::Bytes;
use futures::{Future, FutureExt, StreamExt, TryFutureExt};
use interledger_btp::{connect_to_service_account, BtpAccount, BtpOutgoingService};
use interledger_ccp::{CcpRoutingAccount, Mode, RouteControlRequest, RoutingRelation};
use interledger_errors::*;
use interledger_http::{deserialize_json, HttpAccount, HttpStore};
use interledger_ildcp::IldcpRequest;
use interledger_ildcp::IldcpResponse;
use interledger_rates::ExchangeRateStore;
use interledger_router::RouterStore;
use interledger_service::{
    Account, AccountStore, AddressStore, IncomingService, OutgoingRequest, OutgoingService,
    Username,
};
use interledger_service_util::BalanceStore;
use interledger_settlement::core::{types::SettlementAccount, SettlementClient};
use interledger_spsp::{pay, SpspResponder};
use interledger_stream::{PaymentNotification, StreamNotificationsStore};
use secrecy::{ExposeSecret, SecretString};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::convert::TryFrom;
use std::fmt::Debug;
use tracing::{debug, error, trace};
use uuid::Uuid;
use warp::{self, reply::Json, Filter, Rejection};

pub const BEARER_TOKEN_START: usize = 7;

const fn get_default_max_slippage() -> f64 {
    0.015
}

#[derive(Deserialize, Debug)]
struct SpspPayRequest {
    receiver: String,
    #[serde(deserialize_with = "number_or_string")]
    source_amount: u64,
    #[serde(
        deserialize_with = "number_or_string",
        default = "get_default_max_slippage"
    )]
    slippage: f64,
}

pub fn accounts_api<I, O, S, A, B>(
    server_secret: Bytes,
    admin_api_token: String,
    default_spsp_account: Option<Username>,
    incoming_handler: I,
    outgoing_handler: O,
    btp: BtpOutgoingService<B, A>,
    store: S,
) -> impl warp::Filter<Extract = (impl warp::Reply,), Error = warp::Rejection> + Clone
where
    I: IncomingService<A> + Clone + Send + Sync + 'static,
    O: OutgoingService<A> + Clone + Send + Sync + 'static,
    B: OutgoingService<A> + Clone + Send + Sync + 'static,
    S: NodeStore<Account = A>
        + AccountStore<Account = A>
        + AddressStore
        + HttpStore<Account = A>
        + BalanceStore
        + StreamNotificationsStore<Account = A>
        + ExchangeRateStore
        + RouterStore,
    A: BtpAccount
        + CcpRoutingAccount
        + SettlementAccount
        + Account
        + HttpAccount
        + Serialize
        + Send
        + Sync
        + 'static,
{
    // TODO can we make any of the Filters const or put them in once_cell?
    let with_store = warp::any().map(move || store.clone());
    let with_incoming_handler = warp::any().map(move || incoming_handler.clone());

    // Helper filters
    let admin_auth_header = format!("Bearer {}", admin_api_token);
    let admin_auth_header_clone = admin_auth_header.clone();
    let with_admin_auth_header = warp::any().map(move || admin_auth_header.clone());
    let admin_only = warp::header::<SecretString>("authorization")
        .and_then(move |authorization: SecretString| {
            let admin_auth_header = admin_auth_header_clone.clone();
            async move {
                if authorization.expose_secret() == &admin_auth_header {
                    Ok::<(), Rejection>(())
                } else {
                    Err(Rejection::from(ApiError::unauthorized()))
                }
            }
        })
        // This call makes it so we do not pass on a () value on
        // success to the next filter, it just gets rid of it
        .untuple_one();

    // Converts an account username to an account id or errors out
    let account_username_to_id = warp::path::param::<Username>()
        .and(with_store.clone())
        .and_then(move |username: Username, store: S| async move {
            let id = store.get_account_id_from_username(&username).await?;
            Ok::<_, Rejection>(id)
        });

    let is_authorized_user = move |store: S, path_username: Username, auth_string: SecretString| {
        async move {
            if auth_string.expose_secret().len() < BEARER_TOKEN_START {
                return Err(Rejection::from(ApiError::bad_request()));
            }

            // Try getting the account from the store
            let authorized_account = store
                .get_account_from_http_auth(
                    &path_username,
                    &auth_string.expose_secret()[BEARER_TOKEN_START..],
                )
                .await?;

            // Only return the account if the provided username matched the fetched one
            // This maybe is redundant?
            if &path_username == authorized_account.username() {
                Ok(authorized_account)
            } else {
                Err(ApiError::unauthorized().into())
            }
        }
    };

    // Checks if the account is an admin or if they have provided a valid password
    let admin_or_authorized_user_only = warp::path::param::<Username>()
        .and(warp::header::<SecretString>("authorization"))
        .and(with_store.clone())
        .and(with_admin_auth_header)
        .and_then(
            move |path_username: Username,
                  auth_string: SecretString,
                  store: S,
                  admin_auth_header: String| {
                async move {
                    // If it's an admin, there's no need for more checks
                    if auth_string.expose_secret() == &admin_auth_header {
                        let account_id = store.get_account_id_from_username(&path_username).await?;
                        return Ok(account_id);
                    }
                    let account = is_authorized_user(store, path_username, auth_string).await?;
                    Ok::<Uuid, Rejection>(account.id())
                }
            },
        );

    // Checks if the account has provided a valid password (same as admin-or-auth call, minus one call, can we refactor them together?)
    let authorized_user_only = warp::path::param::<Username>()
        .and(warp::header::<SecretString>("authorization"))
        .and(with_store.clone())
        .and_then(
            move |path_username: Username, auth_string: SecretString, store: S| async move {
                let account = is_authorized_user(store, path_username, auth_string).await?;
                Ok::<A, Rejection>(account)
            },
        );

    // POST /accounts
    let btp_clone = btp.clone();
    let outgoing_handler_clone = outgoing_handler.clone();
    let post_accounts = warp::post()
        .and(warp::path("accounts"))
        .and(warp::path::end())
        .and(admin_only.clone())
        .and(deserialize_json()) // Why does warp::body::json not work?
        .and(with_store.clone())
        .and_then(move |account_details: AccountDetails, store: S| {
            let store_clone = store.clone();
            let handler = outgoing_handler_clone.clone();
            let btp = btp_clone.clone();
            async move {
                let account = store.insert_account(account_details.clone()).await?;

                connect_to_external_services(handler, account.clone(), store_clone, btp).await?;
                Ok::<Json, Rejection>(warp::reply::json(&account))
            }
        });

    // GET /accounts
    let get_accounts = warp::get()
        .and(warp::path("accounts"))
        .and(warp::path::end())
        .and(admin_only.clone())
        .and(with_store.clone())
        .and_then(|store: S| async move {
            let accounts = store.get_all_accounts().await?;
            Ok::<Json, Rejection>(warp::reply::json(&accounts))
        });

    // PUT /accounts/:username
    let btp_clone = btp.clone();
    let outgoing_handler_clone = outgoing_handler.clone();
    let put_account = warp::put()
        .and(warp::path("accounts"))
        .and(account_username_to_id.clone())
        .and(warp::path::end())
        .and(admin_only.clone())
        .and(deserialize_json()) // warp::body::json() is not able to decode this!
        .and(with_store.clone())
        .and_then(move |id: Uuid, account_details: AccountDetails, store: S| {
            let outgoing_handler = outgoing_handler_clone.clone();
            let btp = btp_clone.clone();
            if account_details.ilp_over_btp_incoming_token.is_some() {
                // if the BTP token was provided, assume that it's different
                // from the existing one and drop the connection
                // the saved websocket connection
                // a new one will be initialized in the `connect_to_external_services` call
                btp.close_connection(&id);
            }
            async move {
                let account = store.update_account(id, account_details).await?;
                connect_to_external_services(outgoing_handler, account.clone(), store, btp).await?;

                Ok::<Json, Rejection>(warp::reply::json(&account))
            }
        });

    // GET /accounts/:username
    let get_account = warp::get()
        .and(warp::path("accounts"))
        // takes the username and the authorization header and checks if it's authorized, returns the uid
        .and(admin_or_authorized_user_only.clone())
        .and(warp::path::end())
        .and(with_store.clone())
        .and_then(|id: Uuid, store: S| async move {
            let accounts = store.get_accounts(vec![id]).await?;

            Ok::<Json, Rejection>(warp::reply::json(&accounts[0]))
        });

    // GET /accounts/:username/balance
    let get_account_balance = warp::get()
        .and(warp::path("accounts"))
        // takes the username and the authorization header and checks if it's authorized, returns the uid
        .and(admin_or_authorized_user_only.clone())
        .and(warp::path("balance"))
        .and(warp::path::end())
        .and(with_store.clone())
        .and_then(|id: Uuid, store: S| {
            async move {
                // TODO reduce the number of store calls it takes to get the balance
                let mut accounts = store.get_accounts(vec![id]).await?;
                let account = accounts.pop().unwrap();

                let balance = store.get_balance(account.id()).await?;

                let asset_scale = account.asset_scale();
                let asset_code = account.asset_code().to_owned();
                Ok::<Json, Rejection>(warp::reply::json(&json!({
                    // normalize to the base unit
                    "balance": balance as f64 / 10_u64.pow(asset_scale.into()) as f64,
                    "asset_code": asset_code,
                })))
            }
        });

    // DELETE /accounts/:username
    let btp_clone = btp.clone();
    let delete_account = warp::delete()
        .and(warp::path("accounts"))
        .and(account_username_to_id.clone())
        .and(warp::path::end())
        .and(admin_only.clone())
        .and(with_store.clone())
        .and_then(move |id: Uuid, store: S| {
            let btp = btp_clone.clone();
            async move {
                let account = store.delete_account(id).await?;
                // close the btp connection (if any)
                btp.close_connection(&id);
                Ok::<Json, Rejection>(warp::reply::json(&account))
            }
        });

    // PUT /accounts/:username/settings
    let outgoing_handler_clone = outgoing_handler;
    let put_account_settings = warp::put()
        .and(warp::path("accounts"))
        .and(admin_or_authorized_user_only.clone())
        .and(warp::path("settings"))
        .and(warp::path::end())
        .and(deserialize_json())
        .and(with_store.clone())
        .and_then(move |id: Uuid, settings: AccountSettings, store: S| {
            let btp = btp.clone();
            let outgoing_handler = outgoing_handler_clone.clone();
            async move {
                if settings.ilp_over_btp_incoming_token.is_some() {
                    // if the BTP token was provided, assume that it's different
                    // from the existing one and drop the connection
                    // the saved websocket connection
                    btp.close_connection(&id);
                }
                let modified_account = store.modify_account_settings(id, settings).await?;

                // Since the account was modified, we should also try to
                // connect to the new account:
                connect_to_external_services(
                    outgoing_handler,
                    modified_account.clone(),
                    store,
                    btp,
                )
                .await?;
                Ok::<Json, Rejection>(warp::reply::json(&modified_account))
            }
        });

    // (Websocket) /accounts/:username/payments/incoming
    let incoming_payment_notifications = warp::path("accounts")
        .and(admin_or_authorized_user_only)
        .and(warp::path("payments"))
        .and(warp::path("incoming"))
        .and(warp::path::end())
        .and(warp::ws())
        .and(with_store.clone())
        .map(|id: Uuid, ws: warp::ws::Ws, store: S| {
            ws.on_upgrade(move |ws: warp::ws::WebSocket| {
                let (ws_tx, ws_rx) = ws.split();
                tokio::task::spawn(notify_user(ws_tx, id, store).map(|result| result.unwrap()));
                consume_msg_drain(ws_rx)
            })
        });

    // (Websocket) /payments/incoming
    let all_payment_notifications = warp::path("payments")
        .and(admin_only)
        .and(warp::path("incoming"))
        .and(warp::path::end())
        .and(warp::ws())
        .and(with_store.clone())
        .map(|ws: warp::ws::Ws, store: S| {
            ws.on_upgrade(move |ws: warp::ws::WebSocket| {
                let (ws_tx, ws_rx) = ws.split();
                tokio::task::spawn(notify_all_payments(ws_tx, store).map(|result| result.unwrap()));
                consume_msg_drain(ws_rx)
            })
        });

    // POST /accounts/:username/payments
    let post_payments = warp::post()
        .and(warp::path("accounts"))
        .and(authorized_user_only)
        .and(warp::path("payments"))
        .and(warp::path::end())
        .and(deserialize_json())
        .and(with_incoming_handler)
        .and(with_store.clone())
        .and_then(
            move |account: A, pay_request: SpspPayRequest, incoming_handler: I, store: S| {
                async move {
                    let receipt = pay(
                        incoming_handler,
                        account.clone(),
                        store,
                        &pay_request.receiver,
                        pay_request.source_amount,
                        pay_request.slippage,
                    )
                    .map_err(|err| {
                        let msg = format!("Error sending SPSP payment: {}", err);
                        error!("{}", msg);
                        // TODO give a different error message depending on what type of error it is
                        Rejection::from(ApiError::internal_server_error().detail(msg))
                    })
                    .await?;

                    debug!("Sent SPSP payment, receipt: {:?}", receipt);
                    Ok::<Json, Rejection>(warp::reply::json(&json!(receipt)))
                }
            },
        );

    // GET /accounts/:username/spsp
    let server_secret_clone = server_secret.clone();
    let get_spsp = warp::get()
        .and(warp::path("accounts"))
        .and(account_username_to_id)
        .and(warp::path("spsp"))
        .and(warp::path::end())
        .and(with_store.clone())
        .and_then(move |id: Uuid, store: S| {
            let server_secret_clone = server_secret_clone.clone();
            async move {
                let accounts = store.get_accounts(vec![id]).await?;
                // TODO return the response without instantiating an SpspResponder (use a simple fn)
                Ok::<_, Rejection>(
                    SpspResponder::new(
                        accounts[0].ilp_address().clone(),
                        server_secret_clone.clone(),
                    )
                    .generate_http_response(),
                )
            }
        });

    // GET /.well-known/pay
    // This is the endpoint a [Payment Pointer](https://github.com/interledger/rfcs/blob/master/0026-payment-pointers/0026-payment-pointers.md)
    // with no path resolves to
    let get_spsp_well_known = warp::get()
        .and(warp::path(".well-known"))
        .and(warp::path("pay"))
        .and(warp::path::end())
        .and(with_store)
        .and_then(move |store: S| {
            let default_spsp_account = default_spsp_account.clone();
            let server_secret_clone = server_secret.clone();
            async move {
                if let Some(ref username) = default_spsp_account {
                    let id = store.get_account_id_from_username(username).await?;

                    // TODO this shouldn't take multiple store calls
                    let mut accounts = store.get_accounts(vec![id]).await?;

                    let account = accounts.pop().unwrap();
                    // TODO return the response without instantiating an SpspResponder (use a simple fn)
                    Ok::<_, Rejection>(
                        SpspResponder::new(
                            account.ilp_address().clone(),
                            server_secret_clone.clone(),
                        )
                        .generate_http_response(),
                    )
                } else {
                    Err(Rejection::from(
                        ApiError::not_found().detail("no default spsp account was configured"),
                    ))
                }
            }
        });

    get_spsp
        .or(get_spsp_well_known)
        .or(post_accounts)
        .or(get_accounts)
        .or(put_account)
        .or(delete_account)
        .or(get_account)
        .or(get_account_balance)
        .or(put_account_settings)
        .or(incoming_payment_notifications)
        .or(all_payment_notifications)
        .or(post_payments)
}

async fn consume_msg_drain(mut ws_rx: futures::stream::SplitStream<warp::ws::WebSocket>) {
    while let Some(result) = ws_rx.next().await {
        if let Err(e) = result {
            debug!("consume msg drain read error: {}", e);
            break;
        }
    }
}

fn notify_user(
    ws_tx: futures::stream::SplitSink<warp::ws::WebSocket, warp::ws::Message>,
    id: Uuid,
    store: impl StreamNotificationsStore,
) -> impl Future<Output = Result<(), ()>> {
    let (tx, rx) = futures::channel::mpsc::unbounded::<PaymentNotification>();
    // the client is now subscribed
    store.add_payment_notification_subscription(id, tx);

    // Anytime something is written to tx, it will reach rx
    // and get converted to a warp::ws::Message
    let rx = rx.map(|notification: PaymentNotification| {
        let msg = warp::ws::Message::text(serde_json::to_string(&notification).unwrap());
        Ok(msg)
    });

    // Then it gets forwarded to the client
    rx.forward(ws_tx)
        .map(|result| {
            if let Err(e) = result {
                eprintln!("websocket send error: {}", e);
            }
        })
        .then(futures::future::ok)
}

// Similar to notify_user, but instead of associating an account Uuid with a sender,
// it only assumes control of the store's all payment notification receiver; its messages
// are published alongside account-specific notifications and the dedicated thread
// owns the applicable sender.
fn notify_all_payments(
    ws_tx: futures::stream::SplitSink<warp::ws::WebSocket, warp::ws::Message>,
    store: impl StreamNotificationsStore,
) -> impl Future<Output = Result<(), ()>> {
    let rx = tokio_stream::wrappers::BroadcastStream::new(store.all_payment_subscription());
    let rx = rx.map(|notification: _| {
        let msg = serde_json::to_string(&notification.map_err(|e| e.to_string())).unwrap();
        Ok(warp::ws::Message::text(msg))
    });

    rx.forward(ws_tx)
        .map(|result| {
            if let Err(e) = result {
                eprintln!("websocket send error: {}", e);
            }
        })
        .then(futures::future::ok)
}

async fn get_address_from_parent_and_update_routes<O, A, S>(
    mut service: O,
    parent: A,
    store: S,
) -> Result<(), warp::Rejection>
where
    O: OutgoingService<A> + Clone + Send + Sync + 'static,
    A: CcpRoutingAccount + Clone + Send + Sync + 'static,
    S: NodeStore<Account = A> + AddressStore + Clone + Send + Sync + 'static,
{
    debug!(
        "Getting ILP address from parent account: {} (id: {})",
        parent.username(),
        parent.id()
    );
    let prepare = IldcpRequest {}.to_prepare();
    let fulfill = service
        .send_request(OutgoingRequest {
            from: parent.clone(), // Does not matter what we put here, they will get the account from the HTTP/BTP credentials
            to: parent.clone(),
            prepare,
            original_amount: 0,
        })
        .map_err(|err| {
            let msg = format!("Error getting ILDCP info: {:?}", err);
            error!("{}", msg);
            ApiError::internal_server_error().detail(msg)
        })
        .await?;

    let info = IldcpResponse::try_from(fulfill.into_data().freeze()).map_err(|err| {
        let msg = format!(
            "Unable to parse ILDCP response from fulfill packet: {:?}",
            err
        );
        error!("{}", msg);
        ApiError::internal_server_error().detail(msg)
    })?;
    debug!("Got ILDCP response from parent: {:?}", info);
    let ilp_address = info.ilp_address();

    debug!("ILP address is now: {}", ilp_address);
    // TODO we may want to make this trigger the CcpRouteManager to request
    let prepare = RouteControlRequest {
        mode: Mode::Sync,
        last_known_epoch: 0,
        last_known_routing_table_id: [0; 16],
        features: Vec::new(),
    }
    .to_prepare();

    // Set the parent to be the default route for everything
    // that starts with their global prefix
    store.set_default_route(parent.id()).await?;
    // Update our store's address
    store.set_ilp_address(ilp_address).await?;

    // Get the parent's routes for us
    debug!("Asking for routes from {:?}", parent.clone());
    service
        .send_request(OutgoingRequest {
            from: parent.clone(),
            to: parent.clone(),
            original_amount: prepare.amount(),
            prepare: prepare.clone(),
        })
        .map_err(|err| {
            let msg = format!("Error getting routes from parent: {:?}", err);
            error!("{}", msg);
            ApiError::internal_server_error().detail(msg)
        })
        .await?;

    Ok(())
}

// Helper function which gets called whenever a new account is added or
// modified.
// Performed actions:
// 1. If they have a BTP uri configured: connect to their BTP socket
// 2. If they are a parent:
// 2a. Perform an ILDCP Request to get the address assigned to us by them, and
// update our store's address to that value
// 2b. Perform a RouteControl Request to make them send us any new routes
// 3. If they have a settlement engine endpoitn configured: Make a POST to the
//    engine's account creation endpoint with the account's id
async fn connect_to_external_services<O, A, S, B>(
    service: O,
    account: A,
    store: S,
    btp: BtpOutgoingService<B, A>,
) -> Result<A, warp::reject::Rejection>
where
    O: OutgoingService<A> + Clone + Send + Sync + 'static,
    A: CcpRoutingAccount + BtpAccount + SettlementAccount + Clone + Send + Sync + 'static,
    S: NodeStore<Account = A> + AddressStore + BalanceStore + Clone + Send + Sync + 'static,
    B: OutgoingService<A> + Clone + 'static,
{
    // Try to connect to the account's BTP socket if they have
    // one configured
    if account.get_ilp_over_btp_url().is_some() {
        trace!("Newly inserted account has a BTP URL configured, will try to connect");
        connect_to_service_account(account.clone(), true, btp).await?
    }

    // If we added a parent, get the address assigned to us by
    // them and update all of our routes
    if account.routing_relation() == RoutingRelation::Parent {
        get_address_from_parent_and_update_routes(service, account.clone(), store.clone()).await?;
    }

    // Register the account with the settlement engine
    // if a settlement_engine_url was configured on the account
    // or if there is a settlement engine configured for that
    // account's asset_code
    let default_settlement_engine = store
        .get_asset_settlement_engine(account.asset_code())
        .await?;

    let settlement_engine_url = account
        .settlement_engine_details()
        .map(|details| details.url)
        .or(default_settlement_engine);
    if let Some(se_url) = settlement_engine_url {
        let id = account.id();
        let http_client = SettlementClient::default();
        trace!(
            "Sending account {} creation request to settlement engine: {:?}",
            id,
            se_url.clone()
        );

        let response = http_client
            .create_engine_account(id, se_url.clone())
            .map_err(|err| {
                Rejection::from(ApiError::internal_server_error().detail(err.to_string()))
            })
            .await?;

        if response.status().is_success() {
            trace!("Account {} created on the SE", id);

            // We will pre-fund our account with 0, which will return
            // the current settle_to value
            let (_, amount_to_settle) = store.update_balances_for_fulfill(id, 0u64).await?;

            // prefund the absolute value
            if amount_to_settle > 0 {
                http_client
                    .send_settlement(id, se_url, amount_to_settle, account.asset_scale())
                    .map_err(|err| {
                        Rejection::from(ApiError::internal_server_error().detail(err.to_string()))
                    })
                    .await?;
            }
        } else {
            error!(
                "Error creating account. Settlement engine responded with HTTP code: {}",
                response.status()
            );
        }
    }

    Ok(account)
}

#[cfg(test)]
mod tests {
    use crate::routes::test_helpers::*;
    // TODO: Add test for GET /accounts/:username/spsp and /.well_known

    #[tokio::test]
    async fn only_admin_can_create_account() {
        let api = test_accounts_api();
        let resp = api_call(&api, "POST", "/accounts", "admin", DETAILS.clone()).await;
        assert_eq!(resp.status().as_u16(), 200);

        let resp = api_call(&api, "POST", "/accounts", "wrong", DETAILS.clone()).await;
        assert_eq!(resp.status().as_u16(), 401);
    }

    #[tokio::test]
    async fn only_admin_can_delete_account() {
        let api = test_accounts_api();
        let resp = api_call(&api, "DELETE", "/accounts/alice", "admin", DETAILS.clone()).await;
        assert_eq!(resp.status().as_u16(), 200);

        let resp = api_call(&api, "DELETE", "/accounts/alice", "wrong", DETAILS.clone()).await;
        assert_eq!(resp.status().as_u16(), 401);
    }

    #[tokio::test]
    async fn only_admin_can_modify_whole_account() {
        let api = test_accounts_api();
        let resp = api_call(&api, "PUT", "/accounts/alice", "admin", DETAILS.clone()).await;
        assert_eq!(resp.status().as_u16(), 200);

        let resp = api_call(&api, "PUT", "/accounts/alice", "wrong", DETAILS.clone()).await;
        assert_eq!(resp.status().as_u16(), 401);
    }

    #[tokio::test]
    async fn only_admin_can_get_all_accounts() {
        let api = test_accounts_api();
        let resp = api_call(&api, "GET", "/accounts", "admin", None).await;
        assert_eq!(resp.status().as_u16(), 200);

        let resp = api_call(&api, "GET", "/accounts", "wrong", None).await;
        assert_eq!(resp.status().as_u16(), 401);
    }

    #[tokio::test]
    async fn only_admin_or_user_can_get_account() {
        let api = test_accounts_api();
        let resp = api_call(&api, "GET", "/accounts/alice", "admin", DETAILS.clone()).await;
        assert_eq!(resp.status().as_u16(), 200);

        // TODO: Make this not require the username in the token
        let resp = api_call(&api, "GET", "/accounts/alice", "password", DETAILS.clone()).await;
        assert_eq!(resp.status().as_u16(), 200);

        let resp = api_call(&api, "GET", "/accounts/alice", "wrong", DETAILS.clone()).await;
        assert_eq!(resp.status().as_u16(), 401);
    }

    #[tokio::test]
    async fn only_admin_or_user_can_get_accounts_balance() {
        let api = test_accounts_api();
        let resp = api_call(&api, "GET", "/accounts/alice/balance", "admin", None).await;
        assert_eq!(resp.status().as_u16(), 200);

        // TODO: Make this not require the username in the token
        let resp = api_call(&api, "GET", "/accounts/alice/balance", "password", None).await;
        assert_eq!(resp.status().as_u16(), 200);

        let resp = api_call(&api, "GET", "/accounts/alice/balance", "wrong", None).await;
        assert_eq!(resp.status().as_u16(), 401);
    }

    #[tokio::test]
    async fn only_admin_or_user_can_modify_accounts_settings() {
        let api = test_accounts_api();
        let resp = api_call(
            &api,
            "PUT",
            "/accounts/alice/settings",
            "admin",
            DETAILS.clone(),
        )
        .await;
        assert_eq!(resp.status().as_u16(), 200);

        // TODO: Make this not require the username in the token
        let resp = api_call(
            &api,
            "PUT",
            "/accounts/alice/settings",
            "password",
            DETAILS.clone(),
        )
        .await;
        assert_eq!(resp.status().as_u16(), 200);

        let resp = api_call(
            &api,
            "PUT",
            "/accounts/alice/settings",
            "wrong",
            DETAILS.clone(),
        )
        .await;
        assert_eq!(resp.status().as_u16(), 401);
    }

    #[tokio::test]
    async fn only_admin_or_user_can_send_payment() {
        let payment: Option<serde_json::Value> = Some(serde_json::json!({
            "receiver": "some_receiver",
            "source_amount" : 10,
        }));
        let api = test_accounts_api();
        let resp = api_call(
            &api,
            "POST",
            "/accounts/alice/payments",
            "password",
            payment.clone(),
        )
        .await;
        // This should return an internal server error since we're making an invalid payment request
        // We could have set up a mockito mock to set that pay is called correctly but we merely want
        // to check that authorization and paths work as expected
        assert_eq!(resp.status().as_u16(), 500);

        // Note that the operator has indirect access to the user's token since they control the store
        let resp = api_call(
            &api,
            "POST",
            "/accounts/alice/payments",
            "admin",
            payment.clone(),
        )
        .await;
        assert_eq!(resp.status().as_u16(), 401);

        let resp = api_call(
            &api,
            "POST",
            "/accounts/alice/payments",
            "wrong",
            payment.clone(),
        )
        .await;
        assert_eq!(resp.status().as_u16(), 401);
    }
}
