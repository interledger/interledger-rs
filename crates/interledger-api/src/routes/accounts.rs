use crate::{AccountDetails, NodeStore, BEARER_TOKEN_START};
use futures::{
    future::{err, ok, result, Either},
    Future,
};
use hyper::Response;
use interledger_http::{HttpAccount, HttpStore};
use interledger_service::Account;
use interledger_service_util::BalanceStore;
use log::{debug, error, trace};
use reqwest::r#async::Client;
use serde::Serialize;
use serde_json::{json, Value};
use std::str::FromStr;
use tokio_retry::{strategy::FixedInterval, Retry};
use tower_web::{impl_web, Response};
use url::Url;

#[derive(Serialize, Response, Debug)]
#[web(status = "200")]
struct AccountsResponse<A: Serialize> {
    accounts: Vec<A>,
}

#[derive(Response)]
#[web(status = "200")]
struct Success;

#[derive(Response, Debug)]
#[web(status = "200")]
struct BalanceResponse {
    balance: String,
}

pub struct AccountsApi<T> {
    store: T,
    admin_api_token: String,
}

const MAX_RETRIES: usize = 10;

impl_web! {
    impl<T, A> AccountsApi<T>
    where T: NodeStore<Account = A> + HttpStore<Account = A> + BalanceStore<Account = A>,
    A: Account + HttpAccount + Serialize + 'static,

    {
        pub fn new(admin_api_token: String, store: T) -> Self {
            AccountsApi {
                store,
                admin_api_token,
            }
        }

        fn is_admin(&self, authorization: &str) -> bool {
            authorization[BEARER_TOKEN_START..] == self.admin_api_token
        }

        fn validate_admin(&self, authorization: String) -> impl Future<Item = T, Error = Response<()>> {
            if self.is_admin(&authorization) {
                ok(self.store.clone())
            } else {
                error!("Admin API endpoint called with non-admin API key");
                err(Response::builder().status(401).body(()).unwrap())
            }
        }

        #[post("/accounts")]
        #[content_type("application/json")]
        fn post_accounts(&self, body: AccountDetails, authorization: String) -> impl Future<Item = Value, Error = Response<()>> {
            // TODO don't allow accounts to be overwritten
            // TODO try connecting to that account's websocket server if it has a btp_uri
            let se_url = body.settlement_engine_url.clone();
            self.validate_admin(authorization)
                .and_then(move |store| store.insert_account(body)
                .map_err(|_| Response::builder().status(500).body(()).unwrap())
                .and_then(|account| {
                    // if the account had a SE associated with it, then register
                    // the account in the SE.
                    if let Some(se_url)  = se_url {
                        let id = account.id();
                        Either::A(result(Url::parse(&se_url))
                        .map_err(|_| Response::builder().status(500).body(()).unwrap())
                        .and_then(move |mut se_url| {
                            se_url
                                .path_segments_mut()
                                .expect("Invalid settlement engine URL")
                                .push("accounts");
                            trace!(
                                "Sending account {} creation request to settlement engine: {:?}",
                                id,
                                se_url.clone()
                            );
                            let action = move || {
                                Client::new().post(se_url.clone())
                                .json(&json!({"id" : id.to_string()}))
                                .send()
                                .map_err(move |err| {
                                    error!("Error sending account creation command to the settlement engine: {:?}", err)
                                })
                                .and_then(move |response| {
                                    if response.status().is_success() {
                                        trace!("Account {} created on the SE", id);
                                        Ok(())
                                    } else {
                                        error!("Error creating account. Settlement engine responded with HTTP code: {}", response.status());
                                        Err(())
                                    }
                                })
                            };
                            Retry::spawn(FixedInterval::from_millis(2000).take(MAX_RETRIES), action)
                            .map_err(|_| Response::builder().status(500).body(()).unwrap())
                            .and_then(move |_| {
                                Ok(json!(account))
                            })
                        }))
                    } else {
                        Either::B(ok(json!(account)))
                    }
                }))
        }

        #[get("/accounts")]
        #[content_type("application/json")]
        fn get_accounts(&self, authorization: String) -> impl Future<Item = Value, Error = Response<()>> {
            let store = self.store.clone();
            if self.is_admin(&authorization) {
                Either::A(store.get_all_accounts()
                    .map_err(|_| Response::builder().status(500).body(()).unwrap())
                    .and_then(|accounts| Ok(json!(accounts))))
            } else {
                // Only allow the user to see their own account
                Either::B(store.get_account_from_http_token(authorization.as_str())
                    .map_err(|_| Response::builder().status(404).body(()).unwrap())
                    .and_then(|account| Ok(json!(vec![account]))))
            }
        }

        #[get("/accounts/:id")]
        #[content_type("application/json")]
        fn get_account(&self, id: String, authorization: String) -> impl Future<Item = Value, Error = Response<()>> {
            let store = self.store.clone();
            let is_admin = self.is_admin(&authorization);
            let parsed_id: Result<A::AccountId, ()> = A::AccountId::from_str(&id).map_err(|_| error!("Invalid id"));
            result(parsed_id)
                .map_err(|_| Response::builder().status(400).body(()).unwrap())
                .and_then(move |id| {
                    if is_admin  {
                        Either::A(store.get_accounts(vec![id])
                            .map_err(move |_| {
                                debug!("Account not found: {}", id);
                                Response::builder().status(404).body(()).unwrap()
                            })
                            .and_then(|mut accounts| Ok(json!(accounts.pop().unwrap()))))
                    } else {
                        Either::B(store.get_account_from_http_token(&authorization[BEARER_TOKEN_START..])
                            .map_err(move |_| {
                                debug!("No account found with auth: {}", authorization);
                                Response::builder().status(401).body(()).unwrap()
                            })
                            .and_then(move |account| {
                                if account.id() == id {
                                    Ok(json!(account))
                                } else {
                                    Err(Response::builder().status(401).body(()).unwrap())
                                }
                            }))
                    }
                })
        }

        // TODO should this be combined into the account record?
        #[get("/accounts/:id/balance")]
        #[content_type("application/json")]
        fn get_balance(&self, id: String, authorization: String) -> impl Future<Item = BalanceResponse, Error = Response<()>> {
            let store = self.store.clone();
            let store_clone = self.store.clone();
            let is_admin = self.is_admin(&authorization);
            let parsed_id: Result<A::AccountId, ()> = A::AccountId::from_str(&id).map_err(|_| error!("Invalid id"));
            result(parsed_id)
                .map_err(|_| Response::builder().status(400).body(()).unwrap())
                .and_then(move |id| {
                    if is_admin  {
                        Either::A(store.get_accounts(vec![id])
                            .map_err(move |_| {
                                debug!("Account not found: {}", id);
                                Response::builder().status(404).body(()).unwrap()
                            })
                            .and_then(|mut accounts| Ok(accounts.pop().unwrap())))
                    } else {
                        Either::B(store.get_account_from_http_token(&authorization[BEARER_TOKEN_START..])
                            .map_err(move |_| {
                                debug!("No account found with auth: {}", authorization);
                                Response::builder().status(401).body(()).unwrap()
                            })
                            .and_then(move |account| {
                                if account.id() == id {
                                    Ok(account)
                                } else {
                                    Err(Response::builder().status(401).body(()).unwrap())
                                }
                            }))
                    }
                })
                .and_then(move |account| store_clone.get_balance(account)
                .map_err(|_| Response::builder().status(500).body(()).unwrap()))
                .and_then(|balance| Ok(BalanceResponse {
                    balance: balance.to_string(),
                }))
        }
    }
}
