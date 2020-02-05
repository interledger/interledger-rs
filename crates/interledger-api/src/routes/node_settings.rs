use crate::{ExchangeRates, NodeStore};
use bytes::Bytes;
use futures::TryFutureExt;
use interledger_errors::*;
use interledger_http::{deserialize_json, HttpAccount};
use interledger_packet::Address;
use interledger_router::RouterStore;
use interledger_service::{Account, AccountStore, AddressStore, Username};
use interledger_service_util::ExchangeRateStore;
use interledger_settlement::core::{types::SettlementAccount, SettlementClient};
use log::{error, trace};
use secrecy::{ExposeSecret, SecretString};
use serde::Serialize;
use std::{
    collections::HashMap,
    iter::FromIterator,
    str::{self, FromStr},
};
use url::Url;
use uuid::Uuid;
use warp::{self, reply::Json, Filter, Rejection};

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
        + AccountStore<Account = A>
        + AddressStore
        + ExchangeRateStore
        + RouterStore,
    A: Account + HttpAccount + Send + Sync + SettlementAccount + Serialize + 'static,
{
    // Helper filters
    let admin_auth_header = format!("Bearer {}", admin_api_token);
    let admin_only = warp::header::<SecretString>("authorization")
        .and_then(move |authorization: SecretString| {
            let admin_auth_header = admin_auth_header.clone();
            async move {
                if authorization.expose_secret() == &admin_auth_header {
                    Ok::<(), Rejection>(())
                } else {
                    Err(Rejection::from(
                        ApiError::unauthorized().detail("invalid admin auth token provided"),
                    ))
                }
            }
        })
        // This call makes it so we do not pass on a () value on
        // success to the next filter, it just gets rid of it
        .untuple_one()
        .boxed();
    let with_store = warp::any().map(move || store.clone()).boxed();

    // GET /
    let get_root = warp::get()
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
    let put_rates = warp::put()
        .and(warp::path("rates"))
        .and(warp::path::end())
        .and(admin_only.clone())
        .and(deserialize_json())
        .and(with_store.clone())
        .and_then(|rates: ExchangeRates, store: S| async move {
            store.set_exchange_rates(rates.0.clone())?;
            Ok::<_, Rejection>(warp::reply::json(&rates))
        })
        .boxed();

    // GET /rates
    let get_rates = warp::get()
        .and(warp::path("rates"))
        .and(warp::path::end())
        .and(with_store.clone())
        .and_then(|store: S| async move {
            let rates = store.get_all_exchange_rates()?;
            Ok::<_, Rejection>(warp::reply::json(&rates))
        })
        .boxed();

    // GET /routes
    // Response: Map of ILP Address prefix -> Username
    let get_routes = warp::get()
        .and(warp::path("routes"))
        .and(warp::path::end())
        .and(with_store.clone())
        .and_then(|store: S| {
            async move {
                // Convert the account IDs listed in the routing table
                // to the usernames for the API response
                let routes = store.routing_table().clone();
                let accounts = store
                    .get_accounts(routes.values().cloned().collect())
                    .await?;
                let routes: HashMap<String, String> = HashMap::from_iter(
                    routes
                        .iter()
                        .map(|(prefix, _)| prefix.to_string())
                        .zip(accounts.into_iter().map(|a| a.username().to_string())),
                );

                Ok::<Json, Rejection>(warp::reply::json(&routes))
            }
        })
        .boxed();

    // PUT /routes/static
    // Body: Map of ILP Address prefix -> Username
    let put_static_routes = warp::put()
        .and(warp::path("routes"))
        .and(warp::path("static"))
        .and(warp::path::end())
        .and(admin_only.clone())
        .and(deserialize_json())
        .and(with_store.clone())
        .and_then(move |routes: HashMap<String, String>, store: S| {
            async move {
                // Convert the usernames to account IDs to set the routes in the store
                let mut usernames: Vec<Username> = Vec::new();
                for username in routes.values() {
                    let user = match Username::from_str(&username) {
                        Ok(u) => u,
                        Err(_) => return Err(Rejection::from(ApiError::bad_request())),
                    };
                    usernames.push(user);
                }

                let mut account_ids: Vec<Uuid> = Vec::new();
                for username in usernames {
                    account_ids.push(store.get_account_id_from_username(&username).await?);
                }

                let prefixes = routes.keys().map(|s| s.to_string());
                store
                    .set_static_routes(prefixes.zip(account_ids.into_iter()))
                    .await?;
                Ok::<Json, Rejection>(warp::reply::json(&routes))
            }
        })
        .boxed();

    // PUT /routes/static/:prefix
    // Body: Username
    let put_static_route = warp::put()
        .and(warp::path("routes"))
        .and(warp::path("static"))
        .and(warp::path::param::<String>())
        .and(warp::path::end())
        .and(admin_only.clone())
        .and(warp::body::bytes())
        .and(with_store.clone())
        .and_then(|prefix: String, body: Bytes, store: S| {
            async move {
                let username_str =
                    str::from_utf8(&body).map_err(|_| Rejection::from(ApiError::bad_request()))?;
                let username = Username::from_str(username_str)
                    .map_err(|_| Rejection::from(ApiError::bad_request()))?;
                // Convert the username to an account ID to set it in the store
                let account_id = store.get_account_id_from_username(&username).await?;
                store.set_static_route(prefix, account_id).await?;
                Ok::<String, Rejection>(username.to_string())
            }
        })
        .boxed();

    // PUT /settlement/engines
    let put_settlement_engines = warp::put()
        .and(warp::path("settlement"))
        .and(warp::path("engines"))
        .and(warp::path::end())
        .and(admin_only)
        .and(warp::body::json())
        .and(with_store)
        .and_then(move |asset_to_url_map: HashMap<String, Url>, store: S| async move {
            let asset_to_url_map_clone = asset_to_url_map.clone();
            store
                .set_settlement_engines(asset_to_url_map.clone()).await?;
            // Create the accounts on the settlement engines for any
            // accounts that are using the default settlement engine URLs
            // (This is done in case we modify the globally configured settlement
            // engine URLs after accounts have already been added)

            // TODO we should come up with a better way of ensuring
            // the accounts are created that doesn't involve loading
            // all of the accounts from the database into memory
            // (even if this isn't called often, it could crash the node at some point)
            let accounts = store.get_all_accounts().await?;

            let client = SettlementClient::default();
            // Try creating the account on the settlement engine if the settlement_engine_url of the
            // account is the one we just configured as the default for the account's asset code
            for account in accounts {
                if let Some(details) = account.settlement_engine_details() {
                    if Some(&details.url) == asset_to_url_map.get(account.asset_code()) {
                        let response = client.create_engine_account(account.id(), details.url)
                            .map_err(|err| Rejection::from(ApiError::internal_server_error().detail(err.to_string())))
                            .await?;
                        if response.status().is_success() {
                            trace!("Account {} created on the SE", account.id());
                        } else {
                            error!("Error creating account. Settlement engine responded with HTTP code: {}", response.status());
                        }
                    }
                }
            }
            Ok::<Json, Rejection>(warp::reply::json(&asset_to_url_map_clone))
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

    #[tokio::test]
    async fn gets_status() {
        let api = test_node_settings_api();
        let resp = api_call(&api, "GET", "/", "", None).await;
        assert_eq!(resp.status().as_u16(), 200);
        assert_eq!(
            resp.body(),
            &b"{\"status\":\"Ready\",\"ilp_address\":\"example.connector\"}"[..]
        );
    }

    #[tokio::test]
    async fn gets_rates() {
        let api = test_node_settings_api();
        let resp = api_call(&api, "GET", "/rates", "", None).await;
        assert_eq!(resp.status().as_u16(), 200);
        assert_eq!(
            serde_json::from_slice::<Value>(resp.body()).unwrap(),
            json!({"XYZ":2.0,"ABC":1.0})
        );
    }

    #[tokio::test]
    async fn gets_routes() {
        let api = test_node_settings_api();
        let resp = api_call(&api, "GET", "/routes", "", None).await;
        assert_eq!(resp.status().as_u16(), 200);
    }

    #[tokio::test]
    async fn only_admin_can_put_rates() {
        let api = test_node_settings_api();
        let rates = json!({"ABC": 1.0});
        let resp = api_call(&api, "PUT", "/rates", "admin", Some(rates.clone())).await;
        assert_eq!(resp.status().as_u16(), 200);

        let resp = api_call(&api, "PUT", "/rates", "wrong", Some(rates)).await;
        assert_eq!(resp.status().as_u16(), 401);
    }

    #[tokio::test]
    async fn only_admin_can_put_static_routes() {
        let api = test_node_settings_api();
        let routes = json!({"g.node1": "alice", "example.eu": "bob"});
        let resp = api_call(&api, "PUT", "/routes/static", "admin", Some(routes.clone())).await;
        assert_eq!(resp.status().as_u16(), 200);

        let resp = api_call(&api, "PUT", "/routes/static", "wrong", Some(routes)).await;
        assert_eq!(resp.status().as_u16(), 401);
    }

    #[tokio::test]
    async fn only_admin_can_put_single_static_route() {
        let api = test_node_settings_api();
        let api_put = |auth: String| {
            let auth = format!("Bearer {}", auth);
            async {
                warp::test::request()
                    .method("PUT")
                    .path("/routes/static/g.node1")
                    .body("alice")
                    .header("Authorization", auth)
                    .reply(&api)
                    .await
            }
        };

        let resp = api_put("admin".to_owned()).await;
        assert_eq!(resp.status().as_u16(), 200);

        let resp = api_put("wrong".to_owned()).await;
        assert_eq!(resp.status().as_u16(), 401);
    }

    #[tokio::test]
    async fn only_admin_can_put_engines() {
        let api = test_node_settings_api();
        let engines = json!({"ABC": "http://localhost:3000", "XYZ": "http://localhost:3001"});
        let resp = api_call(
            &api,
            "PUT",
            "/settlement/engines",
            "admin",
            Some(engines.clone()),
        )
        .await;
        assert_eq!(resp.status().as_u16(), 200);

        let resp = api_call(&api, "PUT", "/settlement/engines", "wrong", Some(engines)).await;
        assert_eq!(resp.status().as_u16(), 401);
    }
}
