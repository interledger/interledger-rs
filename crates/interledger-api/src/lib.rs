#![recursion_limit = "128"]
#[macro_use]
extern crate tower_web;
#[macro_use]
extern crate log;

use futures::{future::result, Future};
use http::Response;
use interledger_http::{HttpAccount, HttpStore};
use interledger_service::{Account as AccountTrait, IncomingService};
use interledger_service_util::BalanceStore;
use interledger_spsp::pay;
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

pub struct Node<T, S> {
    store: T,
    incoming_handler: S,
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

impl_web! {
    impl<T, S, A> Node<T, S>
    where T: NodeStore<Account = A> + HttpStore<Account = A> + BalanceStore<Account = A>,
    S: IncomingService<A> + Clone + Send + Sync + 'static,
    A: AccountTrait + HttpAccount + NodeAccount + 'static,

    {
        pub fn new(store: T, incoming_handler: S) -> Self {
            Node {
                store,
                incoming_handler,
            }
        }

        #[post("/accounts")]
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
                            Response::builder().status(500).body(format!("Error sending SPSP payment: {:?}", err)).unwrap()
                        })
                })
        }

        // TODO add quoting via SPSP/STREAM
    }
}
