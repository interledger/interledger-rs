use crate::BEARER_TOKEN_START;
use bytes::Bytes;
use futures::{
    future::{err, result, Either},
    Future,
};
use hyper::{Body, Response};
use interledger_http::{HttpAccount, HttpStore};
use interledger_ildcp::IldcpAccount;
use interledger_service::{AccountStore, IncomingService};
use interledger_spsp::{pay, SpspResponder};
use std::str::FromStr;

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
    default_spsp_account: Option<String>,
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

        pub fn default_spsp_account(&mut self, account_id: String) -> &mut Self {
            self.default_spsp_account = Some(account_id);
            self
        }

        #[post("/pay")]
        #[content_type("application/json")]
        // TODO add a version that lets you specify the destination amount instead
        fn post_pay(&self, body: SpspPayRequest, authorization: String) -> impl Future<Item = SpspPayResponse, Error = Response<String>> {
            let service = self.incoming_handler.clone();
            debug!("Got request to pay: {:?}", body);
            self.store.get_account_from_http_token(&authorization[BEARER_TOKEN_START..])
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
                    // TODO return the response without instantiating an SpspResponder (use a simple fn)
                    Ok(SpspResponder::new(ilp_address, server_secret)
                        .generate_http_response())
                    })
        }



        // TODO resolve payment pointers with subdomains to the correct account
        // also give accounts aliases to use in the payment pointer instead of the ids
        #[get("/.well-known/pay")]
        fn get_well_known(&self) -> impl Future<Item = Response<Body>, Error = Response<()>> {
            if let Some(ref account_id) = &self.default_spsp_account {
                Either::A(self.get_spsp(account_id.to_string()))
            } else {
                error!("Got SPSP request to /.well-known/pay endpoint but there is no default SPSP account configured");
                Either::B(err(Response::builder().status(404).body(()).unwrap()))
            }
        }

        // TODO add quoting via SPSP/STREAM
    }
}
