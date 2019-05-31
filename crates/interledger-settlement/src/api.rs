use crate::{SettlementAccount, SettlementStore};
use futures::{
    future::{err, result, Either},
    Future,
};
use hyper::Response;
use interledger_packet::PrepareBuilder;
use interledger_service::{AccountStore, OutgoingRequest, OutgoingService};
use serde_json::Value;
use std::{
    marker::PhantomData,
    str::{self, FromStr},
    time::{Duration, SystemTime},
};

static PEER_PROTOCOL_CONDITION: [u8; 32] = [
    102, 104, 122, 173, 248, 98, 189, 119, 108, 143, 193, 139, 142, 159, 142, 32, 8, 151, 20, 133,
    110, 226, 51, 179, 144, 42, 89, 29, 13, 95, 41, 37,
];

pub struct SettlementApi<S, T, A> {
    outgoing_handler: S,
    store: T,
    account_type: PhantomData<A>,
}

#[derive(Extract)]
#[serde(rename_all = "camelCase")]
struct SettlementDetails {
    account_id: String,
    amount: u64,
}

#[derive(Response)]
#[web(status = "200")]
struct Success;

// TODO add authentication

impl_web! {
    impl<S, T, A> SettlementApi<S, T, A>
    where
        S: OutgoingService<A> + Clone + Send + 'static,
        T: SettlementStore<Account = A> + AccountStore<Account = A> + Clone + Send + 'static,
        A: SettlementAccount + 'static,
    {
        pub fn new(store: T, outgoing_handler: S) -> Self {
            SettlementApi {
                outgoing_handler,
                store,
                account_type: PhantomData,
            }
        }

        #[post("/settlements/receiveMoney")]
        fn receive_settlement(&self, body: SettlementDetails) -> impl Future<Item = Success, Error = Response<()>> {
            let amount = body.amount;
            let store = self.store.clone();
            let account_id = body.account_id;
            result(A::AccountId::from_str(account_id.as_str())
                .map_err(move |_err| {
                    error!("Unable to parse account id: {}", account_id);
                    Response::builder().status(400).body(()).unwrap()
                }))
                .and_then(move |account_id| {
                    store.update_balance_for_incoming_settlement(account_id, amount)
                        .map_err(move |_| {
                            error!("Error updating balance of account: {} for incoming settlement of amount: {}", account_id, amount);
                            Response::builder().status(500).body(()).unwrap()
                        })
                })
                .and_then(|_| Ok(Success))
        }

        #[post("/settlements/sendMessage")]
        fn send_outgoing_message(&self, body: Value)-> impl Future<Item = Value, Error = Response<()>> {
            if let Value::Object(json) = &body {
                if let Some(account_id) = json.get("accountId").and_then(|a| a.as_str()) {
                    if let Ok(account_id) = A::AccountId::from_str(account_id) {
                        let mut outgoing_handler = self.outgoing_handler.clone();
                        return Either::A(self.store.get_accounts(vec![account_id])
                            .map_err(move |_| {
                                error!("Account {} not found", account_id);
                                Response::builder().status(404).body(()).unwrap()
                            })
                            .and_then(|accounts| {
                                let account = &accounts[0];
                                if let Some(settlement_engine) = account.settlement_engine_details() {
                                    Ok((account.clone(), settlement_engine))
                                } else {
                                    error!("Account {} has no settlement engine details configured, cannot send a settlement engine message to that account", accounts[0].id());
                                    Err(Response::builder().status(404).body(()).unwrap())
                                }
                            })
                            .and_then(move |(account, settlement_engine)| {
                                outgoing_handler.send_request(OutgoingRequest {
                                    from: account.clone(),
                                    to: account.clone(),
                                    original_amount: 0,
                                    prepare: PrepareBuilder {
                                        destination: settlement_engine.ilp_address.as_ref(),
                                        amount: 0,
                                        expires_at: SystemTime::now() + Duration::from_secs(30),
                                        data: body.to_string().as_bytes().as_ref(),
                                        execution_condition: &PEER_PROTOCOL_CONDITION,
                                    }.build()
                                })
                                .map_err(|reject| {
                                    error!("Error sending message to peer settlement engine. Packet rejected with code: {}, message: {}", reject.code(), str::from_utf8(reject.message()).unwrap_or_default());
                                    // TODO should we respond with different HTTP error codes based on the ILP error codes?
                                    Response::builder().status(502).body(()).unwrap()
                                })
                            })
                            .and_then(|fulfill| {
                                serde_json::from_slice(fulfill.data()).map_err(|err| {
                                    error!("Error parsing response from peer settlement engine as JSON: {:?}", err);
                                    Response::builder().status(502).body(()).unwrap()
                                })
                            }));
                    }
                }
            }
            Either::B(err(Response::builder().status(400).body(()).unwrap()))
        }
    }
}
