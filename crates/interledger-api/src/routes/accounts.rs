use crate::{http_retry::Client, number_or_string, AccountDetails, AccountSettings, NodeStore};
use bytes::Bytes;
use futures::{
    future::{err, join_all, ok, Either},
    Future, Stream,
};
use interledger_btp::{connect_to_service_account, BtpAccount, BtpOutgoingService};
use interledger_ccp::{CcpRoutingAccount, Mode, RouteControlRequest, RoutingRelation};
use interledger_http::{deserialize_json, error::*, HttpAccount, HttpStore};
use interledger_ildcp::IldcpRequest;
use interledger_ildcp::IldcpResponse;
use interledger_router::RouterStore;
use interledger_service::{
    Account, AddressStore, IncomingService, OutgoingRequest, OutgoingService, Username,
};
use interledger_service_util::{BalanceStore, ExchangeRateStore};
use interledger_settlement::core::types::SettlementAccount;
use interledger_spsp::{pay, SpspResponder};
use interledger_stream::{PaymentNotification, StreamNotificationsStore};
use log::{debug, error, trace};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::convert::TryFrom;
use uuid::Uuid;
use warp::{self, Filter, Rejection};

pub const BEARER_TOKEN_START: usize = 7;

#[derive(Deserialize, Debug)]
struct SpspPayRequest {
    receiver: String,
    #[serde(deserialize_with = "number_or_string")]
    source_amount: u64,
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
        + HttpStore<Account = A>
        + BalanceStore<Account = A>
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
    // TODO can we make any of the Filters const or put them in lazy_static?

    // Helper filters
    let admin_auth_header = format!("Bearer {}", admin_api_token);
    let admin_only = warp::header::<String>("authorization")
        .and_then(move |authorization| -> Result<(), Rejection> {
            if authorization == admin_auth_header {
                Ok(())
            } else {
                Err(ApiError::unauthorized().into())
            }
        })
        // This call makes it so we do not pass on a () value on
        // success to the next filter, it just gets rid of it
        .untuple_one()
        .boxed();
    let with_store = warp::any().map(move || store.clone()).boxed();
    let admin_auth_header = format!("Bearer {}", admin_api_token);
    let with_admin_auth_header = warp::any().map(move || admin_auth_header.clone()).boxed();
    let with_incoming_handler = warp::any().map(move || incoming_handler.clone()).boxed();
    // Note that the following path filters should be applied before others
    // (such as method and authentication) to avoid triggering unexpected errors for requests
    // that do not match this path.
    let accounts = warp::path("accounts");
    let accounts_index = accounts.and(warp::path::end());
    // This is required when using `admin_or_authorized_user_only` or `authorized_user_only` filter.
    // Sets Username from path into ext for context.
    let account_username = accounts
        .and(warp::path::param2::<Username>())
        .and_then(|username: Username| -> Result<_, Rejection> {
            warp::filters::ext::set(username);
            Ok(())
        })
        .untuple_one()
        .boxed();
    let account_username_to_id = accounts
        .and(warp::path::param2::<Username>())
        .and(with_store.clone())
        .and_then(|username: Username, store: S| {
            store
                .get_account_id_from_username(&username)
                .map_err::<_, Rejection>(move |_| {
                    // TODO differentiate between server error and not found
                    error!("Error getting account id from username: {}", username);
                    ApiError::account_not_found().into()
                })
        })
        .boxed();

    // Receives parameters which were prepared by `account_username` and
    // considers the request is eligible to be processed or not, checking the auth.
    // Why we separate `account_username` and this filter is that
    // we want to check whether the sender is eligible to access this path but at the same time,
    // we don't want to spawn any `Rejection`s at `account_username`.
    // At the point of `account_username`, there might be some other
    // remaining path filters. So we have to process those first, not to spawn errors of
    // unauthorized that the the request actually should not cause.
    // This function needs parameters which can be prepared by `account_username`.
    let admin_or_authorized_user_only = warp::filters::ext::get::<Username>()
        .and(warp::header::<String>("authorization"))
        .and(with_store.clone())
        .and(with_admin_auth_header.clone())
        .and_then(
            |path_username: Username, auth_string: String, store: S, admin_auth_header: String| {
                if auth_string.len() < BEARER_TOKEN_START {
                    return Either::A(err(ApiError::bad_request().into()));
                }
                Either::B(store.get_account_id_from_username(&path_username).then(
                    move |account_id: Result<Uuid, _>| {
                        if account_id.is_err() {
                            return Either::A(err::<Uuid, Rejection>(
                                ApiError::account_not_found().into(),
                            ));
                        }
                        let account_id = account_id.unwrap();
                        if auth_string == admin_auth_header {
                            return Either::A(ok(account_id));
                        }
                        Either::B(
                            store
                                .get_account_from_http_auth(
                                    &path_username,
                                    &auth_string[BEARER_TOKEN_START..],
                                )
                                .then(move |authorized_account: Result<A, _>| {
                                    if authorized_account.is_err() {
                                        return err(ApiError::unauthorized().into());
                                    }
                                    let authorized_account = authorized_account.unwrap();
                                    if &path_username == authorized_account.username() {
                                        ok(authorized_account.id())
                                    } else {
                                        err(ApiError::unauthorized().into())
                                    }
                                }),
                        )
                    },
                ))
            },
        )
        .boxed();

    // The same structure as `admin_or_authorized_user_only`.
    // This function needs parameters which can be prepared by `account_username`.
    let authorized_user_only = warp::filters::ext::get::<Username>()
        .and(warp::header::<String>("authorization"))
        .and(with_store.clone())
        .and_then(|path_username: Username, auth_string: String, store: S| {
            if auth_string.len() < BEARER_TOKEN_START {
                return Either::A(err(ApiError::bad_request().into()));
            }
            Either::B(
                store
                    .get_account_from_http_auth(&path_username, &auth_string[BEARER_TOKEN_START..])
                    .then(move |authorized_account: Result<A, _>| {
                        if authorized_account.is_err() {
                            return err::<A, Rejection>(ApiError::unauthorized().into());
                        }
                        let authorized_account = authorized_account.unwrap();
                        if &path_username == authorized_account.username() {
                            ok(authorized_account)
                        } else {
                            err(ApiError::unauthorized().into())
                        }
                    }),
            )
        })
        .boxed();

    // POST /accounts
    let btp_clone = btp.clone();
    let outgoing_handler_clone = outgoing_handler.clone();
    let post_accounts = warp::post2()
        .and(accounts_index)
        .and(admin_only.clone())
        .and(deserialize_json())
        .and(with_store.clone())
        .and_then(move |account_details: AccountDetails, store: S| {
            let store_clone = store.clone();
            let handler = outgoing_handler_clone.clone();
            let btp = btp_clone.clone();
            store
                .insert_account(account_details.clone())
                .map_err(move |_| {
                    error!("Error inserting account into store: {:?}", account_details);
                    // TODO need more information
                    ApiError::internal_server_error().into()
                })
                .and_then(move |account| {
                    connect_to_external_services(handler, account, store_clone, btp)
                })
                .and_then(|account: A| Ok(warp::reply::json(&account)))
        })
        .boxed();

    // GET /accounts
    let get_accounts = warp::get2()
        .and(accounts_index)
        .and(admin_only.clone())
        .and(with_store.clone())
        .and_then(|store: S| {
            store
                .get_all_accounts()
                .map_err::<_, Rejection>(|_| ApiError::internal_server_error().into())
                .and_then(|accounts| Ok(warp::reply::json(&accounts)))
        })
        .boxed();

    // PUT /accounts/:username
    let put_account = warp::put2()
        .and(account_username_to_id.clone())
        .and(warp::path::end())
        .and(admin_only.clone())
        .and(deserialize_json())
        .and(with_store.clone())
        .and_then(move |id: Uuid, account_details: AccountDetails, store: S| {
            let store_clone = store.clone();
            let handler = outgoing_handler.clone();
            let btp = btp.clone();
            store
                .update_account(id, account_details)
                .map_err::<_, Rejection>(move |_| ApiError::internal_server_error().into())
                .and_then(move |account| {
                    connect_to_external_services(handler, account, store_clone, btp)
                })
                .and_then(|account: A| Ok(warp::reply::json(&account)))
        })
        .boxed();

    // GET /accounts/:username
    let get_account = warp::get2()
        .and(account_username.clone())
        .and(warp::path::end())
        .and(admin_or_authorized_user_only.clone())
        .and(with_store.clone())
        .and_then(|id: Uuid, store: S| {
            store
                .get_accounts(vec![id])
                .map_err::<_, Rejection>(|_| ApiError::account_not_found().into())
                .and_then(|accounts| Ok(warp::reply::json(&accounts[0])))
        })
        .boxed();

    // GET /accounts/:username/balance
    let get_account_balance = warp::get2()
        .and(account_username.clone())
        .and(warp::path("balance"))
        .and(warp::path::end())
        .and(admin_or_authorized_user_only.clone())
        .and(with_store.clone())
        .and_then(|id: Uuid, store: S| {
            // TODO reduce the number of store calls it takes to get the balance
            store
                .get_accounts(vec![id])
                .map_err(|_| warp::reject::not_found())
                .and_then(move |mut accounts| {
                    let account = accounts.pop().unwrap();
                    let acc_clone = account.clone();
                    let asset_scale = acc_clone.asset_scale();
                    let asset_code = acc_clone.asset_code().to_owned();
                    store
                        .get_balance(account)
                        .map_err(move |_| {
                            error!("Error getting balance for account: {}", id);
                            ApiError::internal_server_error().into()
                        })
                        .and_then(move |balance: i64| {
                            Ok(warp::reply::json(&json!({
                                // normalize to the base unit
                                "balance": balance as f64 / 10_u64.pow(asset_scale.into()) as f64,
                                "asset_code": asset_code,
                            })))
                        })
                })
        })
        .boxed();

    // DELETE /accounts/:username
    let delete_account = warp::delete2()
        .and(account_username_to_id.clone())
        .and(warp::path::end())
        .and(admin_only.clone())
        .and(with_store.clone())
        .and_then(|id: Uuid, store: S| {
            store
                .delete_account(id)
                .map_err::<_, Rejection>(move |_| {
                    error!("Error deleting account {}", id);
                    ApiError::internal_server_error().into()
                })
                .and_then(|account| Ok(warp::reply::json(&account)))
        })
        .boxed();

    // PUT /accounts/:username/settings
    let put_account_settings = warp::put2()
        .and(account_username.clone())
        .and(warp::path("settings"))
        .and(warp::path::end())
        .and(admin_or_authorized_user_only.clone())
        .and(deserialize_json())
        .and(with_store.clone())
        .and_then(|id: Uuid, settings: AccountSettings, store: S| {
            store
                .modify_account_settings(id, settings)
                .map_err::<_, Rejection>(move |_| {
                    error!("Error updating account settings {}", id);
                    ApiError::internal_server_error().into()
                })
                .and_then(|settings| Ok(warp::reply::json(&settings)))
        })
        .boxed();

    // (Websocket) /accounts/:username/payments/incoming
    let incoming_payment_notifications = account_username
        .clone()
        .and(warp::path("payments"))
        .and(warp::path("incoming"))
        .and(warp::path::end())
        .and(admin_or_authorized_user_only.clone())
        .and(warp::ws2())
        .and(with_store.clone())
        .map(|id: Uuid, ws: warp::ws::Ws2, store: S| {
            ws.on_upgrade(move |ws: warp::ws::WebSocket| {
                let (tx, rx) = futures::sync::mpsc::unbounded::<PaymentNotification>();
                store.add_payment_notification_subscription(id, tx);
                rx.map_err(|_| -> warp::Error { unreachable!("unbounded rx never errors") })
                    .map(|notification| {
                        warp::ws::Message::text(serde_json::to_string(&notification).unwrap())
                    })
                    .forward(ws)
                    .map(|_| ())
                    .map_err(|err| error!("Error forwarding notifications to websocket: {:?}", err))
            })
        })
        .boxed();

    // POST /accounts/:username/payments
    let post_payments = warp::post2()
        .and(account_username.clone())
        .and(warp::path("payments"))
        .and(warp::path::end())
        .and(authorized_user_only.clone())
        .and(deserialize_json())
        .and(with_incoming_handler.clone())
        .and_then(
            move |account: A, pay_request: SpspPayRequest, incoming_handler: I| {
                pay(
                    incoming_handler,
                    account.clone(),
                    &pay_request.receiver,
                    pay_request.source_amount,
                )
                .and_then(move |receipt| {
                    debug!("Sent SPSP payment, receipt: {:?}", receipt);
                    Ok(warp::reply::json(&json!(receipt)))
                })
                .map_err::<_, Rejection>(|err| {
                    error!("Error sending SPSP payment: {:?}", err);
                    // TODO give a different error message depending on what type of error it is
                    ApiError::internal_server_error().into()
                })
            },
        )
        .boxed();

    // GET /accounts/:username/spsp
    let server_secret_clone = server_secret.clone();
    let get_spsp = warp::get2()
        .and(account_username_to_id.clone())
        .and(warp::path("spsp"))
        .and(warp::path::end())
        .and(with_store.clone())
        .and_then(move |id: Uuid, store: S| {
            let server_secret_clone = server_secret_clone.clone();
            store
                .get_accounts(vec![id])
                .map_err::<_, Rejection>(|_| ApiError::internal_server_error().into())
                .and_then(move |accounts| {
                    // TODO return the response without instantiating an SpspResponder (use a simple fn)
                    Ok(SpspResponder::new(
                        accounts[0].ilp_address().clone(),
                        server_secret_clone.clone(),
                    )
                    .generate_http_response())
                })
        })
        .boxed();

    // GET /.well-known/pay
    // This is the endpoint a [Payment Pointer](https://github.com/interledger/rfcs/blob/master/0026-payment-pointers/0026-payment-pointers.md)
    // with no path resolves to
    let server_secret_clone = server_secret.clone();
    let get_spsp_well_known = warp::get2()
        .and(warp::path(".well-known"))
        .and(warp::path("pay"))
        .and(warp::path::end())
        .and(with_store.clone())
        .and_then(move |store: S| {
            // TODO don't clone this
            if let Some(username) = default_spsp_account.clone() {
                let server_secret_clone = server_secret_clone.clone();
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
                                .map_err(|_| ApiError::internal_server_error().into())
                                .map(|mut accounts| accounts.pop().unwrap())
                        })
                        .and_then(move |account| {
                            // TODO return the response without instantiating an SpspResponder (use a simple fn)
                            Ok(SpspResponder::new(
                                account.ilp_address().clone(),
                                server_secret_clone.clone(),
                            )
                            .generate_http_response())
                        }),
                )
            } else {
                Either::B(err(ApiError::not_found().into()))
            }
        })
        .boxed();

    get_spsp
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
        .boxed()
}

fn get_address_from_parent_and_update_routes<O, A, S>(
    mut service: O,
    parent: A,
    store: S,
) -> impl Future<Item = (), Error = ()>
where
    O: OutgoingService<A> + Clone + Send + Sync + 'static,
    A: CcpRoutingAccount + Clone + Send + Sync + 'static,
    S: NodeStore<Account = A> + Clone + Send + Sync + 'static,
{
    debug!(
        "Getting ILP address from parent account: {} (id: {})",
        parent.username(),
        parent.id()
    );
    let prepare = IldcpRequest {}.to_prepare();
    service
        .send_request(OutgoingRequest {
            from: parent.clone(), // Does not matter what we put here, they will get the account from the HTTP/BTP credentials
            to: parent.clone(),
            prepare,
            original_amount: 0,
        })
        .map_err(|err| error!("Error getting ILDCP info: {:?}", err))
        .and_then(|fulfill| {
            let response = IldcpResponse::try_from(fulfill.into_data().freeze()).map_err(|err| {
                error!(
                    "Unable to parse ILDCP response from fulfill packet: {:?}",
                    err
                );
            });
            debug!("Got ILDCP response from parent: {:?}", response);
            let ilp_address = match response {
                Ok(info) => info.ilp_address(),
                Err(_) => return err(()),
            };
            ok(ilp_address)
        })
        .and_then(move |ilp_address| {
            debug!("ILP address is now: {}", ilp_address);
            // TODO we may want to make this trigger the CcpRouteManager to request
            let prepare = RouteControlRequest {
                mode: Mode::Sync,
                last_known_epoch: 0,
                last_known_routing_table_id: [0; 16],
                features: Vec::new(),
            }
            .to_prepare();
            debug!("Asking for routes from {:?}", parent.clone());
            join_all(vec![
                // Set the parent to be the default route for everything
                // that starts with their global prefix
                store.set_default_route(parent.id()),
                // Update our store's address
                store.set_ilp_address(ilp_address),
                // Get the parent's routes for us
                Box::new(
                    service
                        .send_request(OutgoingRequest {
                            from: parent.clone(),
                            to: parent.clone(),
                            original_amount: prepare.amount(),
                            prepare: prepare.clone(),
                        })
                        .and_then(move |_| Ok(()))
                        .map_err(move |err| {
                            error!("Got error when trying to update routes {:?}", err)
                        }),
                ),
            ])
        })
        .and_then(move |_| Ok(()))
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
fn connect_to_external_services<O, A, S, B>(
    service: O,
    account: A,
    store: S,
    btp: BtpOutgoingService<B, A>,
) -> impl Future<Item = A, Error = warp::reject::Rejection>
where
    O: OutgoingService<A> + Clone + Send + Sync + 'static,
    A: CcpRoutingAccount + BtpAccount + SettlementAccount + Clone + Send + Sync + 'static,
    S: NodeStore<Account = A> + AddressStore + Clone + Send + Sync + 'static,
    B: OutgoingService<A> + Clone + 'static,
{
    // Try to connect to the account's BTP socket if they have
    // one configured
    let btp_connect_fut = if account.get_ilp_over_btp_url().is_some() {
        trace!("Newly inserted account has a BTP URL configured, will try to connect");
        Either::A(
            connect_to_service_account(account.clone(), true, btp)
                .map_err(|_| ApiError::internal_server_error().into()),
        )
    } else {
        Either::B(ok(()))
    };

    btp_connect_fut.and_then(move |_| {
        // If we added a parent, get the address assigned to us by
        // them and update all of our routes
        let get_ilp_address_fut = if account.routing_relation() == RoutingRelation::Parent {
            Either::A(
                get_address_from_parent_and_update_routes(service, account.clone(), store.clone())
                .map_err(|_| ApiError::internal_server_error().into())
            )
        } else {
            Either::B(ok(()))
        };

        let default_settlement_engine_fut = store.get_asset_settlement_engine(account.asset_code())
            .map_err(|_| ApiError::internal_server_error().into());

        // Register the account with the settlement engine
        // if a settlement_engine_url was configured on the account
        // or if there is a settlement engine configured for that
        // account's asset_code
        default_settlement_engine_fut.join(get_ilp_address_fut).and_then(move |(default_settlement_engine, _)| {
            let settlement_engine_url = account.settlement_engine_details().map(|details| details.url).or(default_settlement_engine);
            if let Some(se_url) = settlement_engine_url {
                let id = account.id();
                let http_client = Client::default();
                trace!(
                    "Sending account {} creation request to settlement engine: {:?}",
                    id,
                    se_url.clone()
                );
                Either::A(
                    http_client.create_engine_account(se_url, id)
                    .map_err(|_| ApiError::internal_server_error().into())
                    .and_then(move |status_code| {
                        if status_code.is_success() {
                            trace!("Account {} created on the SE", id);
                        } else {
                            error!("Error creating account. Settlement engine responded with HTTP code: {}", status_code);
                        }
                        Ok(())
                    })
                    .and_then(move |_| {
                        Ok(account)
                    }))
            } else {
                Either::B(ok(account))
            }
        })
    })
}

#[cfg(test)]
mod tests {
    use crate::routes::test_helpers::*;

    #[test]
    fn only_admin_can_create_account() {
        let api = test_accounts_api();
        let resp = api_call(&api, "POST", "/accounts", "admin", DETAILS.clone());
        assert_eq!(resp.status().as_u16(), 200);

        let resp = api_call(&api, "POST", "/accounts", "wrong", DETAILS.clone());
        assert_eq!(resp.status().as_u16(), 401);
    }

    #[test]
    fn only_admin_can_delete_account() {
        let api = test_accounts_api();
        let resp = api_call(&api, "DELETE", "/accounts/alice", "admin", DETAILS.clone());
        assert_eq!(resp.status().as_u16(), 200);

        let resp = api_call(&api, "DELETE", "/accounts/alice", "wrong", DETAILS.clone());
        assert_eq!(resp.status().as_u16(), 401);
    }

    #[test]
    fn only_admin_can_modify_whole_account() {
        let api = test_accounts_api();
        let resp = api_call(&api, "PUT", "/accounts/alice", "admin", DETAILS.clone());
        assert_eq!(resp.status().as_u16(), 200);

        let resp = api_call(&api, "PUT", "/accounts/alice", "wrong", DETAILS.clone());
        assert_eq!(resp.status().as_u16(), 401);
    }

    #[test]
    fn only_admin_can_get_all_accounts() {
        let api = test_accounts_api();
        let resp = api_call(&api, "GET", "/accounts", "admin", DETAILS.clone());
        assert_eq!(resp.status().as_u16(), 200);

        let resp = api_call(&api, "GET", "/accounts", "wrong", DETAILS.clone());
        assert_eq!(resp.status().as_u16(), 401);
    }

    #[test]
    fn only_admin_or_user_can_get_account() {
        let api = test_accounts_api();
        let resp = api_call(&api, "GET", "/accounts/alice", "admin", DETAILS.clone());
        assert_eq!(resp.status().as_u16(), 200);

        // TODO: Make this not require the username in the token
        let resp = api_call(&api, "GET", "/accounts/alice", "password", DETAILS.clone());
        assert_eq!(resp.status().as_u16(), 200);

        let resp = api_call(&api, "GET", "/accounts/alice", "wrong", DETAILS.clone());
        assert_eq!(resp.status().as_u16(), 401);
    }

    #[test]
    fn only_admin_or_user_can_get_accounts_balance() {
        let api = test_accounts_api();
        let resp = api_call(
            &api,
            "GET",
            "/accounts/alice/balance",
            "admin",
            DETAILS.clone(),
        );
        assert_eq!(resp.status().as_u16(), 200);

        // TODO: Make this not require the username in the token
        let resp = api_call(
            &api,
            "GET",
            "/accounts/alice/balance",
            "password",
            DETAILS.clone(),
        );
        assert_eq!(resp.status().as_u16(), 200);

        let resp = api_call(
            &api,
            "GET",
            "/accounts/alice/balance",
            "wrong",
            DETAILS.clone(),
        );
        assert_eq!(resp.status().as_u16(), 401);
    }

    #[test]
    fn only_admin_or_user_can_modify_accounts_settings() {
        let api = test_accounts_api();
        let resp = api_call(
            &api,
            "PUT",
            "/accounts/alice/settings",
            "admin",
            DETAILS.clone(),
        );
        assert_eq!(resp.status().as_u16(), 200);

        // TODO: Make this not require the username in the token
        let resp = api_call(
            &api,
            "PUT",
            "/accounts/alice/settings",
            "password",
            DETAILS.clone(),
        );
        assert_eq!(resp.status().as_u16(), 200);

        let resp = api_call(
            &api,
            "PUT",
            "/accounts/alice/settings",
            "wrong",
            DETAILS.clone(),
        );
        assert_eq!(resp.status().as_u16(), 401);
    }
}
