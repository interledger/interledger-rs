use crate::{http_retry::Client, ApiError, ExchangeRates, NodeStore};
use bytes::Buf;
use futures::{
    future::{err, join_all, Either},
    Future,
};
use interledger_http::{HttpAccount, HttpStore};
use interledger_router::RouterStore;
use interledger_service::Account;
use interledger_service_util::{BalanceStore, ExchangeRateStore};
use interledger_settlement::SettlementAccount;
use log::{error, trace};
use serde::Serialize;
use serde_json::json;
use std::{
    collections::HashMap,
    iter::FromIterator,
    str::{self, FromStr},
};
use url::Url;
use warp::{self, Filter};

pub fn node_settings_api<S, A>(
    admin_api_token: String,
    store: S,
) -> warp::filters::BoxedFilter<(impl warp::Reply,)>
where
    S: NodeStore<Account = A>
        + HttpStore<Account = A>
        + BalanceStore<Account = A>
        + ExchangeRateStore
        + RouterStore,
    A: Account + HttpAccount + SettlementAccount + Serialize + 'static,
{
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

    // GET /
    let get_root = warp::get2()
        .and(warp::path::end())
        .and(with_store.clone())
        .map(|store: S| {
            // TODO add more to this response
            warp::reply::json(&json!({
                "status": "Ready".to_string(),
                "ilp_address": store.get_ilp_address(),
            }))
        })
        .boxed();

    // PUT /rates
    let put_rates = warp::put2()
        .and(admin_only.clone())
        .and(warp::path("rates"))
        .and(warp::path::end())
        .and(warp::body::json())
        .and(with_store.clone())
        .and_then(|rates: ExchangeRates, store: S| {
            if store.set_exchange_rates(rates.0.clone()).is_ok() {
                Ok(warp::reply::json(&rates))
            } else {
                error!("Error setting exchange rates");
                Err(warp::reject::custom(ApiError::InternalServerError))
            }
        })
        .boxed();

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
                Err(warp::reject::custom(ApiError::InternalServerError))
            }
        })
        .boxed();

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
        })
        .boxed();

    // PUT /routes/static
    let put_static_routes = warp::put2()
        .and(admin_only.clone())
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
                    return Either::B(err(warp::reject::custom(ApiError::BadRequest)));
                }
            }
            Either::A(
                store
                    .set_static_routes(parsed.clone())
                    .map_err(|_| {
                        error!("Error setting static routes");
                        warp::reject::custom(ApiError::InternalServerError)
                    })
                    .map(move |_| warp::reply::json(&parsed)),
            )
        })
        .boxed();

    // PUT /routes/static/:prefix
    let put_static_route = warp::put2()
        .and(admin_only.clone())
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
                                warp::reject::custom(ApiError::InternalServerError)
                            })
                            .map(move |_| id.to_string()),
                    );
                }
            }
            error!("Body was not a valid Account ID");
            Either::B(err(warp::reject::custom(ApiError::BadRequest)))
        })
        .boxed();

    // PUT /settlement/engines
    let put_settlement_engines = warp::put2()
        .and(admin_only.clone())
        .and(warp::path("settlement"))
        .and(warp::path("engines"))
        .and(warp::path::end())
        .and(warp::body::json())
        .and(with_store.clone())
        .and_then(|asset_to_url_map: HashMap<String, Url>, store: S| {
            let asset_to_url_map_clone = asset_to_url_map.clone();
            store
                .set_settlement_engines(asset_to_url_map.clone())
                .map_err(|_| {
                    error!("Error setting settlement engines");
                    warp::reject::custom(ApiError::InternalServerError)
                })
                .and_then(move |_| {
                    // Create the accounts on the settlement engines for any
                    // accounts that are using the default settlement engine URLs
                    // (This is done in case we modify the globally configured settlement
                    // engine URLs after accounts have already been added)

                    // TODO we should come up with a better way of ensuring
                    // the accounts are created that doesn't involve loading
                    // all of the accounts from the database into memory
                    // (even if this isn't called often, it could crash the node at some point)
                    store.get_all_accounts()
                        .map_err(|_| {
                            warp::reject::custom(ApiError::InternalServerError)
                        })
                    .and_then(move |accounts| {
                        let client = Client::default();
                        let create_settlement_accounts =
                            accounts.into_iter().filter_map(move |account| {
                                let id = account.id();
                                // Try creating the account on the settlement engine if the settlement_engine_url of the
                                // account is the one we just configured as the default for the account's asset code
                                if let Some(details) = account.settlement_engine_details() {
                                    if Some(&details.url) == asset_to_url_map.get(account.asset_code()) {
                                        return Some(client.create_engine_account(details.url, account.id())
                                            .map_err(|_| {
                                                warp::reject::custom(ApiError::InternalServerError)
                                            })
                                            .and_then(move |status_code| {
                                                if status_code.is_success() {
                                                    trace!("Account {} created on the SE", id);
                                                } else {
                                                    error!("Error creating account. Settlement engine responded with HTTP code: {}", status_code);
                                                }
                                                Ok(())
                                            }));
                                        }
                                }
                                None
                            });
                        join_all(create_settlement_accounts)
                    })
                })
                .and_then(move |_| Ok(warp::reply::json(&asset_to_url_map_clone)))
        })
        .boxed();

    get_root
        .or(put_rates)
        .or(get_rates)
        .or(get_routes)
        .or(put_static_routes)
        .or(put_static_route)
        .or(put_settlement_engines)
        .boxed()
}
