use bytes::Bytes;
use futures::{
    future::{err, result, Either},
    Future,
};
use hyper::{Body, Response};
use interledger_http::{HttpAccount, HttpStore};
use interledger_ildcp::IldcpAccount;
use interledger_service::{AccountStore, AuthToken, IncomingService, Username};
use interledger_spsp::{pay, SpspResponder};
use log::{debug, error};
use serde::{Deserialize, Serialize};
use std::str::FromStr;
use tower_web::{impl_web, Extract, Response};

#[derive(Extract, Debug)]
struct SpspPayRequest {
    receiver: String,
    source_amount: u64,
}

#[derive(Response, Debug)]
#[web(status = "200")]
struct SpspPayResponse {
    delivered_amount: u64,
}

#[derive(Response, Debug)]
#[web(status = "200")]
struct SpspQueryResponse {
    destination_account: String,
    shared_secret: String,
}

pub struct SpspApi<T, S> {
    store: T,
    default_spsp_account: Option<Username>,
    incoming_handler: S,
    server_secret: Bytes,
}

impl_web! {
    impl<T, S, A> SpspApi<T, S>
    where T: HttpStore<Account = A> + AccountStore<Account = A>,
    S: IncomingService<A> + Clone + Send + Sync + 'static,
    A: IldcpAccount + HttpAccount + 'static,
    {
        pub fn new(server_secret: Bytes, store: T, incoming_handler: S) -> Self {
            SpspApi {
                store,
                default_spsp_account: None,
                incoming_handler,
                server_secret,
            }
        }

        pub fn default_spsp_account(&mut self, username: Username) -> &mut Self {
            self.default_spsp_account = Some(username);
            self
        }

        #[post("/pay")]
        #[content_type("application/json")]
        // TODO add a version that lets you specify the destination amount instead
        fn post_pay(&self, body: SpspPayRequest, authorization: String) -> impl Future<Item = SpspPayResponse, Error = Response<String>> {
            let service = self.incoming_handler.clone();
            let store = self.store.clone();

            result(AuthToken::from_str(&authorization))
            .map_err(|err| {
                let error_msg = format!("Could not convert auth token {:?}", err);
                error!("{}", error_msg);
                Response::builder().status(500).body(error_msg).unwrap()
            })
            .and_then(move |auth| {
                let username = auth.username();
                let token = auth.password();
                debug!("Got request to pay: {:?}", body);
                store.get_account_from_http_auth(&username, &token)
                .map_err(|_| Response::builder().status(401).body("Unauthorized".to_string()).unwrap())
                .and_then(move |account| {
                    pay(service, account, &body.receiver, body.source_amount)
                        .and_then(|delivered_amount| {
                            debug!("Sent SPSP payment and delivered: {} of the receiver's units", delivered_amount);
                            Ok(SpspPayResponse {
                                delivered_amount,
                                })
                            })
                            .map_err(|err| {
                                error!("Error sending SPSP payment: {:?}", err);
                                // TODO give a different error message depending on what type of error it is
                                Response::builder().status(500).body(format!("Error sending SPSP payment: {:?}", err)).unwrap()
                            })
                    })
            })
        }

        #[get("/spsp/:username")]
        fn get_spsp(&self, username: String) -> impl Future<Item = Response<Body>, Error = Response<()>> {
            let server_secret = self.server_secret.clone();
            let store = self.store.clone();
            result(Username::from_str(&username))
            .map_err(move |_| {
                error!("Invalid username: {}", username);
                Response::builder().status(500).body(()).unwrap()
            })
            .and_then(move |username| {
            store.get_account_id_from_username(&username)
            .map_err(move |_| {
                error!("Error getting account id from username: {}", username);
                Response::builder().status(500).body(()).unwrap()
            })
            .and_then(move |id| store.get_accounts(vec![id])
                .map_err(move |_| {
                    error!("Account not found: {}", id);
                    Response::builder().status(404).body(()).unwrap()
                }))
                .and_then(move |accounts| {
                    // TODO return the response without instantiating an SpspResponder (use a simple fn)
                    Ok(SpspResponder::new(accounts[0].client_address().clone(), server_secret)
                        .generate_http_response())
                    })
            })
        }



        // TODO resolve payment pointers with subdomains to the correct account
        // also give accounts aliases to use in the payment pointer instead of the ids
        #[get("/.well-known/pay")]
        fn get_well_known(&self) -> impl Future<Item = Response<Body>, Error = Response<()>> {
            if let Some(ref username) = &self.default_spsp_account {
                Either::A(self.get_spsp(username.to_string()))
            } else {
                error!("Got SPSP request to /.well-known/pay endpoint but there is no default SPSP account configured");
                Either::B(err(Response::builder().status(404).body(()).unwrap()))
            }
        }

        // TODO add quoting via SPSP/STREAM
    }
}
