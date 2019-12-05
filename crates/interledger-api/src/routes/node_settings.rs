use crate::{http_retry::Client, ExchangeRates, NodeStore};
use bytes::Buf;
use futures::{
    future::{err, join_all, Either},
    Future,
};
use interledger_http::{deserialize_json, error::*, HttpAccount, HttpStore};
use interledger_packet::Address;
use interledger_router::RouterStore;
use interledger_service::{Account, Username};
use interledger_service_util::{BalanceStore, ExchangeRateStore};
use interledger_settlement::core::types::SettlementAccount;
use log::{error, trace};
use serde::Serialize;
use std::{
    collections::HashMap,
    iter::FromIterator,
    str::{self, FromStr},
};
use url::Url;
use warp::{self, Filter, Rejection};

// TODO add more to this response
#[derive(Clone, Serialize)]
struct StatusResponse {
    status: String,
    ilp_address: Address,
    #[serde(skip_serializing_if = "Option::is_none")]
    version: Option<String>,
}

pub fn node_settings_api<S, A>(
    admin_api_token: String,
    node_version: Option<String>,
    store: S,
) -> impl warp::Filter<Extract = (impl warp::Reply,), Error = warp::Rejection> + Clone
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

    // GET /
    let get_root = warp::get2()
        .and(warp::path::end())
        .and(with_store.clone())
        .map(move |store: S| {
            warp::reply::json(&StatusResponse {
                status: "Ready".to_string(),
                ilp_address: store.get_ilp_address(),
                version: node_version.clone(),
            })
        })
        .boxed();

    // PUT /rates
    let put_rates = warp::put2()
        .and(warp::path("rates"))
        .and(warp::path::end())
        .and(admin_only.clone())
        .and(deserialize_json())
        .and(with_store.clone())
        .and_then(|rates: ExchangeRates, store: S| -> Result<_, Rejection> {
            if store.set_exchange_rates(rates.0.clone()).is_ok() {
                Ok(warp::reply::json(&rates))
            } else {
                error!("Error setting exchange rates");
                Err(ApiError::internal_server_error().into())
            }
        })
        .boxed();

    // GET /rates
    let get_rates = warp::get2()
        .and(warp::path("rates"))
        .and(warp::path::end())
        .and(with_store.clone())
        .and_then(|store: S| -> Result<_, Rejection> {
            if let Ok(rates) = store.get_all_exchange_rates() {
                Ok(warp::reply::json(&rates))
            } else {
                error!("Error getting exchange rates");
                Err(ApiError::internal_server_error().into())
            }
        })
        .boxed();

    // GET /routes
    // Response: Map of ILP Address prefix -> Username
    let get_routes = warp::get2()
        .and(warp::path("routes"))
        .and(warp::path::end())
        .and(with_store.clone())
        .and_then(|store: S| {
            // Convert the account IDs listed in the routing table
            // to the usernames for the API response
            let routes = store.routing_table().clone();
            store
                .get_accounts(routes.values().cloned().collect())
                .map_err::<_, Rejection>(|_| {
                    error!("Error getting accounts from store");
                    ApiError::internal_server_error().into()
                })
                .and_then(move |accounts| {
                    let routes: HashMap<String, String> = HashMap::from_iter(
                        routes
                            .iter()
                            .map(|(prefix, _)| prefix.to_string())
                            .zip(accounts.into_iter().map(|a| a.username().to_string())),
                    );

                    Ok(warp::reply::json(&routes))
                })
        })
        .boxed();

    // PUT /routes/static
    // Body: Map of ILP Address prefix -> Username
    let put_static_routes = warp::put2()
        .and(warp::path("routes"))
        .and(warp::path("static"))
        .and(warp::path::end())
        .and(admin_only.clone())
        .and(deserialize_json())
        .and(with_store.clone())
        .and_then(|routes: HashMap<String, Username>, store: S| {
            // Convert the usernames to account IDs to set the routes in the store
            let store_clone = store.clone();
            let usernames: Vec<Username> = routes.values().cloned().collect();
            // TODO use one store call to look up all of the usernames
            join_all(usernames.into_iter().map(move |username| {
                store_clone
                    .get_account_id_from_username(&username)
                    .map_err(move |_| {
                        error!("No account exists with username: {}", username);
                        ApiError::account_not_found().into()
                    })
            }))
            .and_then(move |account_ids| {
                let prefixes = routes.keys().map(|s| s.to_string());
                store
                    .set_static_routes(prefixes.zip(account_ids.into_iter()))
                    .map_err::<_, Rejection>(|_| {
                        error!("Error setting static routes");
                        ApiError::internal_server_error().into()
                    })
                    .map(move |_| warp::reply::json(&routes))
            })
        })
        .boxed();

    // PUT /routes/static/:prefix
    // Body: Username
    let put_static_route = warp::put2()
        .and(warp::path("routes"))
        .and(warp::path("static"))
        .and(warp::path::param2::<String>())
        .and(warp::path::end())
        .and(admin_only.clone())
        .and(warp::body::concat())
        .and(with_store.clone())
        .and_then(|prefix: String, body: warp::body::FullBody, store: S| {
            if let Ok(username) = str::from_utf8(body.bytes())
                .map_err(|_| ())
                .and_then(|string| Username::from_str(string).map_err(|_| ()))
            {
                // Convert the username to an account ID to set it in the store
                let username_clone = username.clone();
                Either::A(
                    store
                        .clone()
                        .get_account_id_from_username(&username)
                        .map_err(move |_| {
                            error!("No account exists with username: {}", username_clone);
                            ApiError::account_not_found().into()
                        })
                        .and_then(move |account_id| {
                            store
                                .set_static_route(prefix, account_id)
                                .map_err::<_, Rejection>(|_| {
                                    error!("Error setting static route");
                                    ApiError::internal_server_error().into()
                                })
                        })
                        .map(move |_| username.to_string()),
                )
            } else {
                Either::B(err(ApiError::bad_request().into()))
            }
        })
        .boxed();

    // PUT /settlement/engines
    let put_settlement_engines = warp::put2()
        .and(warp::path("settlement"))
        .and(warp::path("engines"))
        .and(warp::path::end())
        .and(admin_only.clone())
        .and(deserialize_json())
        .and(with_store.clone())
        .and_then(|asset_to_url_map: HashMap<String, Url>, store: S| {
            let asset_to_url_map_clone = asset_to_url_map.clone();
            store
                .set_settlement_engines(asset_to_url_map.clone())
                .map_err::<_, Rejection>(|_| {
                    error!("Error setting settlement engines");
                    ApiError::internal_server_error().into()
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
                        .map_err(|_| ApiError::internal_server_error().into())
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
                                            .map_err(|_| ApiError::internal_server_error().into())
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

#[cfg(test)]
mod tests {
    use crate::routes::test_helpers::{api_call, test_node_settings_api};
    use serde_json::{json, Value};

    #[test]
    fn gets_status() {
        let api = test_node_settings_api();
        let resp = api_call(&api, "GET", "/", "", None);
        assert_eq!(resp.status().as_u16(), 200);
        assert_eq!(
            resp.body(),
            &b"{\"status\":\"Ready\",\"ilp_address\":\"example.connector\"}"[..]
        );
    }

    #[test]
    fn gets_rates() {
        let api = test_node_settings_api();
        let resp = api_call(&api, "GET", "/rates", "", None);
        assert_eq!(resp.status().as_u16(), 200);
        assert_eq!(
            serde_json::from_slice::<Value>(resp.body()).unwrap(),
            json!({"XYZ":2.0,"ABC":1.0})
        );
    }

    #[test]
    fn gets_routes() {
        let api = test_node_settings_api();
        let resp = api_call(&api, "GET", "/routes", "", None);
        assert_eq!(resp.status().as_u16(), 200);
    }

    #[test]
    fn only_admin_can_put_rates() {
        let api = test_node_settings_api();
        let rates = json!({"ABC": 1.0});
        let resp = api_call(&api, "PUT", "/rates", "admin", Some(rates.clone()));
        assert_eq!(resp.status().as_u16(), 200);

        let resp = api_call(&api, "PUT", "/rates", "wrong", Some(rates));
        assert_eq!(resp.status().as_u16(), 401);
    }

    #[test]
    fn only_admin_can_put_static_routes() {
        let api = test_node_settings_api();
        let routes = json!({"g.node1": "alice", "example.eu": "bob"});
        let resp = api_call(&api, "PUT", "/routes/static", "admin", Some(routes.clone()));
        assert_eq!(resp.status().as_u16(), 200);

        let resp = api_call(&api, "PUT", "/routes/static", "wrong", Some(routes));
        assert_eq!(resp.status().as_u16(), 401);
    }

    #[test]
    fn only_admin_can_put_single_static_route() {
        let api = test_node_settings_api();
        let api_put = move |auth: &str| {
            warp::test::request()
                .method("PUT")
                .path("/routes/static/g.node1")
                .body("alice")
                .header("Authorization", format!("Bearer {}", auth.to_string()))
                .reply(&api)
        };

        let resp = api_put("admin");
        assert_eq!(resp.status().as_u16(), 200);

        let resp = api_put("wrong");
        assert_eq!(resp.status().as_u16(), 401);
    }

    #[test]
    fn only_admin_can_put_engines() {
        let api = test_node_settings_api();
        let engines = json!({"ABC": "http://localhost:3000", "XYZ": "http://localhost:3001"});
        let resp = api_call(
            &api,
            "PUT",
            "/settlement/engines",
            "admin",
            Some(engines.clone()),
        );
        assert_eq!(resp.status().as_u16(), 200);

        let resp = api_call(&api, "PUT", "/settlement/engines", "wrong", Some(engines));
        assert_eq!(resp.status().as_u16(), 401);
    }
}
