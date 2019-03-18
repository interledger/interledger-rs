#![recursion_limit = "128"]
#[macro_use]
extern crate tower_web;
#[macro_use]
extern crate log;

use bytes::Bytes;
use futures::{future::result, Future};
use http::{Request, Response};
use hyper::{body::Body, error::Error};
use interledger_http::{HttpAccount, HttpServerService, HttpStore};
use interledger_ildcp::IldcpAccount;
use interledger_service::{Account as AccountTrait, IncomingService};
use interledger_service_util::BalanceStore;
use interledger_spsp::{pay, SpspResponder};
use std::str::FromStr;

pub trait NodeAccount: HttpAccount {
    fn is_admin(&self) -> bool;
}

pub trait NodeStore: Clone + Send + Sync + 'static {
    type Account: AccountTrait;

    fn insert_account(
        &self,
        account: AccountDetails,
    ) -> Box<Future<Item = Self::Account, Error = ()> + Send>;

    fn set_rates<R>(&self, rates: R) -> Box<Future<Item = (), Error = ()> + Send>
    where
        R: IntoIterator<Item = (String, f64)>;
}

/// The Account type for the RedisStore.
#[derive(Debug, Extract, Response)]
pub struct AccountDetails {
    pub ilp_address: Vec<u8>,
    pub asset_code: String,
    pub asset_scale: u8,
    pub max_packet_amount: u64,
    pub http_endpoint: Option<String>,
    pub http_incoming_authorization: Option<String>,
    pub http_outgoing_authorization: Option<String>,
    pub btp_uri: Option<String>,
    pub btp_incoming_authorization: Option<String>,
    pub is_admin: bool,
}

#[derive(Response)]
#[web(status = "200")]
struct ServerStatus {
    status: String,
}

#[derive(Response)]
#[web(status = "201")]
struct AccountResponse {
    id: String,
}

#[derive(Response)]
#[web(status = "200")]
struct Success;

#[derive(Extract)]
struct Rates(Vec<(String, f64)>);

#[derive(Response)]
#[web(status = "200")]
struct BalanceResponse {
    balance: String,
}

#[derive(Extract)]
struct SpspPayRequest {
    receiver: String,
    source_amount: u64,
}

#[derive(Response)]
#[web(status = "200")]
struct SpspPayResponse {
    amount_delivered: u64,
}

#[derive(Response)]
#[web(status = "200")]
struct SpspQueryResponse {
    destination_account: String,
    shared_secret: String,
}

pub struct NodeApi<T, S> {
    store: T,
    incoming_handler: S,
    server_secret: Bytes,
}

impl_web! {
    impl<T, S, A> NodeApi<T, S>
    where T: NodeStore<Account = A> + HttpStore<Account = A> + BalanceStore<Account = A>,
    S: IncomingService<A> + Clone + Send + Sync + 'static,
    A: AccountTrait + HttpAccount + NodeAccount + IldcpAccount + 'static,

    {
        pub fn new(server_secret: Bytes, store: T, incoming_handler: S) -> Self {
            NodeApi {
                store,
                incoming_handler,
                server_secret,
            }
        }

        #[get("/")]
        #[content_type("application/json")]
        fn get_root(&self) -> Result<ServerStatus, ()> {
            Ok(ServerStatus {
                status: "Ready".to_string(),
            })
        }

        #[post("/accounts")]
        #[content_type("application/json")]
        fn post_accounts(&self, body: AccountDetails, authorization: String) -> impl Future<Item = AccountResponse, Error = Response<()>> {
            let store = self.store.clone();
            self.store.get_account_from_http_auth(&authorization)
                .and_then(|account| if account.is_admin() {
                    Ok(())
                } else {
                    Err(())
                })
                .map_err(|_| Response::builder().status(401).body(()).unwrap())
                .and_then(move |_| store.insert_account(body)
                // TODO make all Accounts (de)serializable with Serde so all the details can be returned here
                .and_then(|account| Ok(AccountResponse {
                    id: format!("{}", account.id())
                }))
                .map_err(|_| Response::builder().status(500).body(()).unwrap()))
        }

        #[get("/accounts/:id/balance")]
        #[content_type("application/json")]
        fn get_balance(&self, id: String, authorization: String) -> impl Future<Item = BalanceResponse, Error = Response<()>> {
            let store = self.store.clone();
            let parsed_id: Result<A::AccountId, ()> = A::AccountId::from_str(&id).map_err(|_| error!("Invalid id"));
            result(parsed_id)
                .map_err(|_| Response::builder().status(400).body(()).unwrap())
                .and_then(move |id| {
                    store.clone().get_account_from_http_auth(&authorization)
                        .and_then(move |account| if account.is_admin() || account.id() == id {
                            Ok(())
                        } else {
                            Err(())
                        })
                        .map_err(|_| Response::builder().status(401).body(()).unwrap())
                        .and_then(move |_| store.get_balance(id)
                        .and_then(|balance| Ok(BalanceResponse {
                            balance: balance.to_string(),
                        }))
                        .map_err(|_| Response::builder().status(404).body(()).unwrap()))
                })
        }

        #[post("/rates")]
        #[content_type("application/json")]
        fn post_rates(&self, body: Rates, authorization: String) -> impl Future<Item = Success, Error = Response<()>> {
            let store = self.store.clone();
            self.store.get_account_from_http_auth(&authorization)
                .and_then(|account| if account.is_admin() {
                    Ok(())
                } else {
                    Err(())
                })
                .map_err(|_| Response::builder().status(401).body(()).unwrap())
                .and_then(move |_| store.set_rates(body.0)
                .and_then(|_| Ok(Success))
                .map_err(|err| {
                    error!("Error setting rates: {:?}", err);
                    Response::builder().status(500).body(()).unwrap()
                }))
        }

        #[post("/pay")]
        #[content_type("application/json")]
        // TODO add a version that lets you specify the destination amount instead
        fn post_pay(&self, body: SpspPayRequest, authorization: String) -> impl Future<Item = SpspPayResponse, Error = Response<String>> {
            let service = self.incoming_handler.clone();
            self.store.get_account_from_http_auth(&authorization)
                .map_err(|_| Response::builder().status(401).body("Unauthorized".to_string()).unwrap())
                .and_then(move |account| {
                    pay(service, account, &body.receiver, body.source_amount)
                        .and_then(|amount_delivered| Ok(SpspPayResponse {
                                amount_delivered,
                            }))
                        .map_err(|err| {
                            error!("Error sending SPSP payment: {:?}", err);
                            // TODO give a different error message depending on what type of error it is
                            Response::builder().status(500).body(format!("Error sending SPSP payment: {:?}", err)).unwrap()
                        })
                })
        }

        #[post("/ilp")]
        // TODO make sure taking the body as a Vec (instead of Bytes) doesn't cause a copy
        // for some reason, it complains that Extract isn't implemented for Bytes even though tower-web says it is
        fn post_ilp(&self, body: Vec<u8>, authorization: String) -> impl Future<Item = Response<Body>, Error = Error> {
            let request = Request::builder()
                .header("Authorization", authorization)
                .body(Body::from(body))
                .unwrap();
            HttpServerService::new(self.incoming_handler.clone(), self.store.clone()).handle_http_request(request)
        }

        #[get("/spsp/:id")]
        fn get_spsp(&self, id: String) -> impl Future<Item = Response<Body>, Error = Response<()>> {
            let server_secret = self.server_secret.clone();
            let store = self.store.clone();
            let id: Result<A::AccountId, ()> = A::AccountId::from_str(&id).map_err(|_| error!("Invalid id: {}", id));
            result(id)
                .map_err(|_| Response::builder().status(400).body(()).unwrap())
                .and_then(move |id| store.get_accounts(vec![id])
                .map_err(move |_| {
                    error!("Account not found: {}", id);
                    Response::builder().status(404).body(()).unwrap()
                }))
                .and_then(move |accounts| {
                    let ilp_address = Bytes::from(accounts[0].client_address());
                    Ok(SpspResponder::new(ilp_address, server_secret)
                        .generate_http_response_from_tag(""))
                    })
        }

        // TODO resolve payment pointers with subdomains to the correct account
        // also give accounts aliases to use in the payment pointer instead of the ids
        #[get("/.well-known/pay")]
        fn get_well_known(&self) -> impl Future<Item = Response<Body>, Error = Response<()>> {
            let default_account = A::AccountId::default();
            let server_secret = self.server_secret.clone();
            self.store.get_accounts(vec![default_account])
            .map_err(move |_| {
                error!("Account not found: {}", default_account);
                Response::builder().status(404).body(()).unwrap()
            })
            .and_then(move |accounts| {
                let ilp_address = Bytes::from(accounts[0].client_address());
                Ok(SpspResponder::new(ilp_address, server_secret)
                    .generate_http_response_from_tag(""))
                })
        }

        // TODO add quoting via SPSP/STREAM
    }
}
