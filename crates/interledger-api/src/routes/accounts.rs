use crate::{http_retry::Client, AccountDetails, AccountSettings, ApiError, NodeStore};
use bytes::Bytes;
use futures::{
    future::{err, ok, result, Either},
    Future, Stream,
};
use interledger_http::{HttpAccount, HttpStore};
use interledger_ildcp::IldcpAccount;
use interledger_router::RouterStore;
use interledger_service::{Account, AuthToken, IncomingService, Username};
use interledger_service_util::{BalanceStore, ExchangeRateStore};
use interledger_spsp::{pay, SpspResponder};
use interledger_stream::{PaymentNotification, StreamNotificationsStore};
use log::{debug, error, trace};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::time::Duration;
use url::Url;
use warp::{self, Filter};

const MAX_RETRIES: usize = 10;
const DEFAULT_HTTP_TIMEOUT: Duration = Duration::from_millis(5000);

#[derive(Deserialize, Debug)]
struct SpspPayRequest {
    receiver: String,
    source_amount: u64,
}

pub fn accounts_api<I, S, A>(
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
        + StreamNotificationsStore<Account = A>
        + ExchangeRateStore
        + RouterStore,
    A: Account + IldcpAccount + HttpAccount + Serialize + 'static,
{
    // TODO can we make any of the Filters const or put them in lazy_static?

    // Helper filters
    let admin_auth_header = format!("Bearer {}", admin_api_token);
    let admin_only = warp::header::<String>("authorization")
        .and_then(move |authorization| -> Result<(), warp::Rejection> {
            if authorization == admin_auth_header {
                Ok(())
            } else {
                Err(warp::reject::custom(ApiError::Unauthorized))
            }
        })
        // This call makes it so we do not pass on a () value on
        // success to the next filter, it just gets rid of it
        .untuple_one()
        .boxed();
    let with_store = warp::any().map(move || store.clone()).boxed();
    let with_incoming_handler = warp::any().map(move || incoming_handler.clone()).boxed();
    let accounts = warp::path("accounts");
    let accounts_index = accounts.and(warp::path::end());
    let account_username = accounts.and(warp::path::param2::<Username>());
    let account_username_to_id = account_username
        .and(with_store.clone())
        .and_then(|username: Username, store: S| {
            store
                .get_account_id_from_username(&username)
                .map_err(move |_| {
                    // TODO differentiate between server error and not found
                    error!("Error getting account id from username: {}", username);
                    warp::reject::custom(ApiError::AccountNotFound)
                })
        })
        .boxed();
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
                    warp::reject::custom(ApiError::Unauthorized)
                })
        })
        .boxed();
    let authorized_account_from_path = account_username
        .and(valid_account_authorization.clone())
        .and_then(
            |path_username: Username, authorized_account: A| -> Result<A, warp::Rejection> {
                // Check that the user is authorized for this route
                if &path_username == authorized_account.username() {
                    Ok(authorized_account)
                } else {
                    Err(warp::reject::custom(ApiError::Unauthorized))
                }
            },
        )
        .boxed();
    let admin_or_authorized_account = admin_only
        .clone()
        .and(account_username_to_id.clone())
        .clone()
        .or(authorized_account_from_path
            .clone()
            .map(|account: A| account.id()))
        .unify()
        .boxed();

    // POST /accounts
    let http_client = Client::new(DEFAULT_HTTP_TIMEOUT, MAX_RETRIES);
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
                    warp::reject::custom(ApiError::InternalServerError)
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
                                warp::reject::custom(ApiError::BadRequest)
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
                                .map_err(|_| {
                                    warp::reject::custom(ApiError::InternalServerError)
                                })
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
        }).boxed();

    // GET /accounts
    let get_accounts = warp::get2()
        .and(accounts_index)
        .and(admin_only.clone())
        .and(with_store.clone())
        .and_then(|store: S| {
            store
                .get_all_accounts()
                .map_err(|_| warp::reject::custom(ApiError::InternalServerError))
                .and_then(|accounts| Ok(warp::reply::json(&accounts)))
        })
        .boxed();

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
                    .map_err(move |_| warp::reject::custom(ApiError::InternalServerError))
                    .and_then(move |account| Ok(warp::reply::json(&account)))
            },
        )
        .boxed();

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
        })
        .boxed();

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
                            warp::reject::custom(ApiError::InternalServerError)
                        })
                })
                .and_then(|balance: i64| {
                    Ok(warp::reply::json(&json!({
                        "balance": balance.to_string(),
                    })))
                })
        })
        .boxed();

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
                    warp::reject::custom(ApiError::InternalServerError)
                })
                .and_then(|account| Ok(warp::reply::json(&account)))
        })
        .boxed();

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
                    warp::reject::custom(ApiError::InternalServerError)
                })
                .and_then(|settings| Ok(warp::reply::json(&settings)))
        })
        .boxed();

    // (Websocket) /accounts/:username/payments/incoming
    let incoming_payment_notifications = warp::ws2()
        .and(admin_or_authorized_account.clone())
        .and(warp::path("payments"))
        .and(warp::path("incoming"))
        .and(warp::path::end())
        .and(with_store.clone())
        .map(|ws: warp::ws::Ws2, id: A::AccountId, store: S| {
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
                    warp::reject::custom(ApiError::InternalServerError)
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
        .and_then(move |id: A::AccountId, store: S| {
            let server_secret_clone = server_secret_clone.clone();
            store
                .get_accounts(vec![id])
                .map_err(|_| warp::reject::custom(ApiError::InternalServerError))
                .and_then(move |accounts| {
                    // TODO return the response without instantiating an SpspResponder (use a simple fn)
                    Ok(SpspResponder::new(
                        accounts[0].client_address().clone(),
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
                                .map_err(|_| warp::reject::custom(ApiError::InternalServerError))
                                .map(|mut accounts| accounts.pop().unwrap())
                        })
                        .and_then(move |account| {
                            // TODO return the response without instantiating an SpspResponder (use a simple fn)
                            Ok(SpspResponder::new(
                                account.client_address().clone(),
                                server_secret_clone.clone(),
                            )
                            .generate_http_response())
                        }),
                )
            } else {
                Either::B(err(warp::reject::not_found()))
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
