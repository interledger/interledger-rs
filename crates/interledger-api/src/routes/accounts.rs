use crate::{AccountDetails, NodeStore, BEARER_TOKEN_START};
use futures::{
    future::{err, ok, result, Either},
    Future,
};
use hyper::Response;
use interledger_http::{HttpAccount, HttpStore};
use interledger_service::Account;
use interledger_service_util::BalanceStore;
use serde::Serialize;
use serde_json::Value;
use std::str::FromStr;

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

        fn validate_admin(&self, authorization: String) -> impl Future<Item = T, Error = Response<()>> {
            if &authorization[BEARER_TOKEN_START..] == self.admin_api_token {
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
            self.validate_admin(authorization)
                .and_then(move |store| store.insert_account(body)
                .and_then(|account| Ok(json!(account)))
                .map_err(|_| Response::builder().status(500).body(()).unwrap()))
        }

        #[get("/accounts")]
        #[content_type("application/json")]
        fn get_accounts(&self, authorization: String) -> impl Future<Item = Value, Error = Response<()>> {
            let store = self.store.clone();
            if authorization == self.admin_api_token {
                    Either::A(store.get_all_accounts()
                        .map_err(|_| Response::builder().status(500).body(()).unwrap())
                .and_then(|accounts| Ok(json!(accounts))))
            } else {
                    Either::B(store.get_account_from_http_token(authorization.as_str())
                        .map_err(|_| Response::builder().status(404).body(()).unwrap())
                        .and_then(|account| Ok(json!(vec![account]))))
            }
        }

        #[get("/accounts/:id")]
        #[content_type("application/json")]
        fn get_account(&self, id: String, authorization: String) -> impl Future<Item = Value, Error = Response<()>> {
            let store = self.store.clone();
            let store_clone = store.clone();
            let is_admin = authorization == self.admin_api_token;
            let parsed_id: Result<A::AccountId, ()> = A::AccountId::from_str(&id).map_err(|_| error!("Invalid id"));
            result(parsed_id)
                .map_err(|_| Response::builder().status(400).body(()).unwrap())
                .and_then(move |id| {
                    store.clone().get_account_from_http_token(&authorization[BEARER_TOKEN_START..])
                        .and_then(move |account|
                            if account.id() == id {
                                Either::A(ok(json!(account)))
                            } else if is_admin {
                                Either::B(store_clone.get_accounts(vec![id]).and_then(|accounts| Ok(json!(accounts[0].clone()))))
                            } else {
                                Either::A(err(()))
                            })
                        .map_err(move |_| {
                            debug!("No account found with auth: {}", authorization);
                            Response::builder().status(401).body(()).unwrap()
                        })
                })
        }

        // TODO should this be combined into the account record?
        #[get("/accounts/:id/balance")]
        #[content_type("application/json")]
        fn get_balance(&self, id: String, authorization: String) -> impl Future<Item = BalanceResponse, Error = Response<()>> {
            let store = self.store.clone();
            let store_clone = store.clone();
            let parsed_id: Result<A::AccountId, ()> = A::AccountId::from_str(&id).map_err(|_| error!("Invalid id"));
            let is_admin = authorization == self.admin_api_token;
            result(parsed_id)
                .map_err(|_| Response::builder().status(400).body(()).unwrap())
                .and_then(move |id| {
                    store.clone().get_account_from_http_token(&authorization[BEARER_TOKEN_START..])
                        .and_then(move |account|
                            if account.id() == id {
                                Either::A(ok(account))
                            } else if is_admin {
                                Either::B(store_clone.get_accounts(vec![id]).and_then(|accounts| Ok(accounts[0].clone())))
                            } else {
                                Either::A(err(()))
                            })
                        .map_err(move |_| {
                            debug!("No account found with auth: {}", authorization);
                            Response::builder().status(401).body(()).unwrap()
                        })
                        .and_then(move |account| store.get_balance(account)
                        .and_then(|balance| Ok(BalanceResponse {
                            balance: balance.to_string(),
                        }))
                        .map_err(|_| Response::builder().status(404).body(()).unwrap()))
                })
        }
    }
}
