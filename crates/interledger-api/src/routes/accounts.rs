use crate::client::Client;
use crate::{AccountDetails, NodeStore, BEARER_TOKEN_START};
use futures::{
    future::{err, ok, result, Either},
    Future,
};
use hyper::Response;
use interledger_http::{HttpAccount, HttpStore};
use interledger_service::{Account, AuthToken, Username};
use interledger_service_util::BalanceStore;
use log::{debug, error, trace};
use serde::Serialize;
use serde_json::{json, Value};
use std::str::FromStr;
use std::time::Duration;
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

#[derive(Clone)]
pub struct AccountsApi<T> {
    store: T,
    admin_api_token: String,
}

const MAX_RETRIES: usize = 10;

// Convenience function to clean up error handling and reduce unwrap quantity
trait ErrorStatus {
    fn error(code: u16) -> Self;
}

impl ErrorStatus for Response<()> {
    fn error(code: u16) -> Self {
        Response::builder().status(code).body(()).unwrap()
    }
}

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
                err(Response::error(401))
            }
        }

        #[post("/accounts")]
        #[content_type("application/json")]
        fn http_post_accounts(&self, body: AccountDetails, authorization: String) -> impl Future<Item = Value, Error = Response<()>> {
            // TODO don't allow accounts to be overwritten
            // TODO try connecting to that account's websocket server if it has
            // a btp_uri
            let se_url = body.settlement_engine_url.clone();
            self.validate_admin(authorization)
                .and_then(move |store| store.insert_account(body)
                .map_err(|_| Response::error(500))
                .and_then(|account| {
                    // if the account had a SE associated with it, then register
                    // the account in the SE.
                    if let Some(se_url)  = se_url {
                        let id = account.id();
                        Either::A(result(Url::parse(&se_url))
                        .map_err(|_| Response::error(500))
                        .and_then(move |se_url| {
                            let client = Client::new(Duration::from_millis(5000), MAX_RETRIES);
                            trace!(
                                "Sending account {} creation request to settlement engine: {:?}",
                                id,
                                se_url.clone()
                            );
                            client.create_engine_account(se_url, id)
                            .and_then(move |status_code| {
                                if status_code.is_success() {
                                    trace!("Account {} created on the SE", id);
                                } else {
                                    error!("Error creating account. Settlement engine responded with HTTP code: {}", status_code);
                                }
                                Ok(())
                            })
                            .map_err(|_| Response::error(500))
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
        fn http_get_accounts(&self, authorization: String) -> impl Future<Item = Value, Error = Response<()>> {
            let store = self.store.clone();
            if self.is_admin(&authorization) {
                Either::A(store.get_all_accounts()
                    .map_err(|_| Response::error(500))
                    .and_then(|accounts| Ok(json!(accounts))))
            } else {
                // Only allow the user to see their own account
                Either::B(result(AuthToken::from_str(&authorization))
                    .map_err(move |_| {
                        error!("No account found with auth: {}", authorization);
                        Response::error(401)
                    })
                    .and_then(move |auth| {
                        store.get_account_from_http_auth(&auth.username(), &auth.password()).map_err(|_| Response::error(401))
                        .and_then(|account| Ok(json!(vec![account])))
                    })
                )
            }
        }

        #[get("/accounts/:username")]
        #[content_type("application/json")]
        fn http_get_account(&self, username: String, authorization: String) -> impl Future<Item = Value, Error = Response<()>> {
            let store = self.store.clone();
            let is_admin = self.is_admin(&authorization);
            let username_clone = username.clone();
            let auth_clone = authorization.clone();
            // TODO:
            // It seems somewhat inconsistent that get_account_from_http_auth
            // would return the Account while this method only gives the ID (and
            // then the next call is made to get the account)
            // One option would be to have two methods get_account_from_username
            // and get_account_from_id (or just get_account) that would provide
            // two ways to get the full Account details. If the next call only
            // takes the account ID, that would be fine because you could just
            // get that ID from the Account
            result(Username::from_str(&username))
            .map_err(move |_| {
                error!("Invalid username: {}", username);
                Response::builder().status(500).body(()).unwrap()
            })
            .and_then(move |username| {
            store.get_account_id_from_username(&username)
            .map_err(move |_| {
                error!("Error getting account id from username: {}", username_clone);
                Response::builder().status(404).body(()).unwrap()
            })
            .and_then(move |id| {
                if is_admin  {
                    Either::A(store.get_accounts(vec![id])
                    .map_err(move |_| {
                        debug!("Account not found: {:?}", id);
                        Response::error(404)
                    })
                    .and_then(|mut accounts| Ok(json!(accounts.pop().unwrap()))))
                } else {
                    Either::B(result(AuthToken::from_str(&auth_clone))
                        .map_err(move |_| {
                            error!("Could not parse auth token {:?}", auth_clone);
                            Response::error(401)
                        })
                        .and_then(move |auth| {
                            store.get_account_from_http_auth(&auth.username(), &auth.password())
                            .map_err(move |_| {
                                debug!("No account found with auth: {}", authorization);
                                Response::error(401)
                            })
                            .and_then(move |account| {
                                if account.id() == id {
                                    Ok(json!(account))
                                } else {
                                    Err(Response::error(401))
                                }
                            })
                        })
                    )
                }
            })
            })
        }

        #[delete("/accounts/:username")]
        #[content_type("application/json")]
        fn http_delete_account(&self, username: String, authorization: String) -> impl Future<Item = Value, Error = Response<()>> {
            let store_clone = self.store.clone();
            let self_clone = self.clone();
            result(Username::from_str(&username))
            .map_err(move |_| {
                error!("Invalid username: {}", username);
                Response::builder().status(500).body(()).unwrap()
            })
            .and_then(move |username| {
                store_clone.get_account_id_from_username(&username)
                .map_err(move |_| {
                    error!("Error getting account id from username: {}", username);
                    Response::builder().status(500).body(()).unwrap()
                })
                .and_then(move |id| {
                    self_clone.validate_admin(authorization)
                    .and_then(move |store| Ok((store, id)))
                    .and_then(move |(store, id)|
                        store.delete_account(id)
                            .map_err(move |_| Response::error(500))
                            .and_then(move |account| {
                                // TODO: deregister from SE if url is present
                                Ok(json!(account))
                            })
                    )
                })
            })
        }

        #[put("/accounts/:username")]
        #[content_type("application/json")]
        fn http_put_account(&self, username: String, body: AccountDetails, authorization: String) -> impl Future<Item = Value, Error = Response<()>> {
            let self_clone = self.clone();
            result(Username::from_str(&username))
            .map_err(move |_| {
                error!("Invalid username: {}", username);
                Response::builder().status(500).body(()).unwrap()
            })
            .and_then(move |username| {
            self_clone.store.get_account_id_from_username(&username)
            .map_err(move |_| {
                error!("Error getting account id from username: {}", username);
                Response::builder().status(500).body(()).unwrap()
            })
            .and_then(move |id| {
                let id = id.to_owned();
                self_clone.validate_admin(authorization)
                .and_then(move |store|
                    store.update_account(id, body)
                        .map_err(move |_| Response::error(500))
                        .and_then(move |account| {
                            Ok(json!(account))
                        })
                )
            })
            })

        }

        // TODO should this be combined into the account record?
        #[get("/accounts/:username/balance")]
        #[content_type("application/json")]
        fn http_get_balance(&self, username: String, authorization: String) -> impl Future<Item = BalanceResponse, Error = Response<()>> {
            let store = self.store.clone();
            let store_clone = self.store.clone();
            let is_admin = self.is_admin(&authorization);
            let username_clone = username.clone();
            let auth_clone = authorization.clone();
            result(Username::from_str(&username))
            .map_err(move |_| {
                error!("Invalid username: {}", username);
                Response::builder().status(500).body(()).unwrap()
            })
            .and_then(move |username| {
            store_clone.get_account_id_from_username(&username)
            .map_err(move |_| {
                error!("Error getting account id from username: {}", username_clone);
                Response::builder().status(500).body(()).unwrap()
            })
            .and_then(move |id| {
                if is_admin  {
                    Either::A(store.get_accounts(vec![id])
                        .map_err(move |_| {
                            debug!("Account not found: {}", id);
                            Response::error(404)
                        })
                        .and_then(|mut accounts| Ok(accounts.pop().unwrap())))
                } else {
                    Either::B(result(AuthToken::from_str(&auth_clone))
                        .map_err(move |_| {
                            error!("Could not parse auth token {:?}", auth_clone);
                            Response::error(401)
                        })
                        .and_then(move |auth| {
                            store.get_account_from_http_auth(&auth.username(), &auth.password())
                            .map_err(move |_| {
                                error!("No account found with auth: {}", authorization);
                                Response::error(401)
                            })
                            .and_then(move |account| {
                                if account.id() == id {
                                    Ok(account)
                                } else {
                                    Err(Response::error(401))
                                }
                            })
                        })
                    )
                }
            })
            .and_then(move |account| store_clone.get_balance(account)
            .map_err(|_| Response::error(500)))
            .and_then(|balance| Ok(BalanceResponse {
                balance: balance.to_string(),
            }))
            })
        }
    }
}
