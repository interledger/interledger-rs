use crate::{NodeStore, BEARER_TOKEN_START};
use futures::{
    future::{err, ok},
    Future,
};
use hyper::Response;
use interledger_router::RouterStore;
use interledger_service::Account;
use interledger_service_util::ExchangeRateStore;
use std::{
    collections::HashMap,
    iter::FromIterator,
    str::{self, FromStr},
};

#[derive(Response, Debug)]
#[web(status = "200")]
struct ServerStatus {
    status: String,
}

#[derive(Response)]
#[web(status = "200")]
struct Success;

#[derive(Extract, Debug)]
struct Rates(Vec<(String, f64)>);

#[derive(Extract, Response, Debug)]
#[web(status = "200")]
struct Routes(HashMap<String, String>);

pub struct SettingsApi<T> {
    store: T,
    admin_api_token: String,
}

impl_web! {
    impl<T, A> SettingsApi<T>
    where T: NodeStore<Account = A> + RouterStore + ExchangeRateStore,
    A: Account + 'static,

    {
        pub fn new(admin_api_token: String, store: T) -> Self {
            SettingsApi {
                store,
                admin_api_token,
            }
        }

        fn validate_admin(&self, authorization: String) -> impl Future<Item = T, Error = Response<()>> {
            if &authorization[BEARER_TOKEN_START..] == self.admin_api_token {
                ok(self.store.clone())
            } else {
                error!("Admin API endpoint called with non-admin API key");
                err(Response::builder().status(401).body(()).unwrap())
            }
        }

        #[get("/")]
        #[content_type("application/json")]
        fn get_root(&self) -> Result<ServerStatus, ()> {
            Ok(ServerStatus {
                status: "Ready".to_string(),
            })
        }

        #[put("/rates")]
        #[content_type("application/json")]
        fn post_rates(&self, body: Rates, authorization: String) -> impl Future<Item = Success, Error = Response<()>> {
            debug!("Setting exchange rates: {:?}", body);
            self.validate_admin(authorization)
                .and_then(move |store| {
                    store.set_rates(body.0)
                        .and_then(|_| Ok(Success))
                        .map_err(|err| {
                            error!("Error setting rates: {:?}", err);
                            Response::builder().status(500).body(()).unwrap()
                        })
                })
        }

        #[get("/routes")]
        #[content_type("application/json")]
        fn get_routes(&self) -> impl Future<Item = Routes, Error = Response<()>> {
            ok(Routes(HashMap::from_iter(self.store.routing_table()
                .into_iter()
                .filter_map(|(address, account)| {
                    if let Ok(address) = str::from_utf8(address.as_ref()) {
                        Some((address.to_string(), account.to_string()))
                    } else {
                        None
                    }
                }))))
        }

        #[put("/routes/static")]
        #[content_type("application/json")]
        fn post_static_routes(&self, body: Routes, authorization: String) -> impl Future<Item = Success, Error = Response<()>> {
            self.validate_admin(authorization)
                .and_then(move |store| {
                    let mut routes: HashMap<String, A::AccountId> = HashMap::with_capacity(body.0.len());
                    for (prefix, account_id) in body.0 {
                        if let Ok(account_id) = A::AccountId::from_str(account_id.as_str()) {
                            routes.insert(prefix, account_id);
                        } else {
                            return Err(Response::builder().status(400).body(()).unwrap());
                        }
                    }
                    Ok((store, routes))
                })
                .and_then(|(store, routes)| {
                    store.set_static_routes(routes)
                    .and_then(|_| Ok(Success))
                        .map_err(|err| {
                            error!("Error setting static routes: {:?}", err);
                            Response::builder().status(500).body(()).unwrap()
                        })
            })
        }

        #[put("/routes/static/:prefix")]
        #[content_type("application/json")]
        fn post_static_route(&self, prefix: String, body: String, authorization: String) -> impl Future<Item = Success, Error = Response<()>> {
            self.validate_admin(authorization)
                .and_then(move |store| {
                    if let Ok(account_id) = A::AccountId::from_str(body.as_str()) {
                        Ok((store, account_id))
                    } else {
                        Err(Response::builder().status(400).body(()).unwrap())
                    }
                })
                .and_then(move |(store, account_id)| {
                    store.set_static_route(prefix, account_id)
                    .and_then(|_| Ok(Success))
                        .map_err(|err| {
                            error!("Error setting static route: {:?}", err);
                            Response::builder().status(500).body(()).unwrap()
                        })
                })
        }

    }
}
