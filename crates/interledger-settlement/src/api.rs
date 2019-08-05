use super::{
    Convert, ConvertDetails, IdempotentData, IdempotentStore, Quantity, SettlementAccount,
    SettlementStore, SE_ILP_ADDRESS,
};
use bigint::uint::U256 as BigU256;
use bytes::Bytes;
use futures::{
    future::{err, ok, result, Either},
    Future,
};
use hyper::{Response, StatusCode};
use interledger_ildcp::IldcpAccount;
use interledger_packet::PrepareBuilder;
use interledger_service::{AccountStore, OutgoingRequest, OutgoingService};
use log::error;
use ring::digest::{digest, SHA256};
use std::{
    marker::PhantomData,
    str::{self, FromStr},
    time::{Duration, SystemTime},
};
use tokio::executor::spawn;
use tower_web::{net::ConnectionStream, ServiceBuilder};

static PEER_PROTOCOL_CONDITION: [u8; 32] = [
    102, 104, 122, 173, 248, 98, 189, 119, 108, 143, 193, 139, 142, 159, 142, 32, 8, 151, 20, 133,
    110, 226, 51, 179, 144, 42, 89, 29, 13, 95, 41, 37,
];

#[derive(Clone)]
pub struct SettlementApi<S, O, A> {
    outgoing_handler: O,
    store: S,
    account_type: PhantomData<A>,
}

impl_web! {
    impl<S, O, A> SettlementApi<S, O, A>
    where
        S: SettlementStore<Account = A> + IdempotentStore + AccountStore<Account = A> + Clone + Send + Sync + 'static,
        O: OutgoingService<A> + Clone + Send + Sync + 'static,
        A: SettlementAccount + IldcpAccount + Send + Sync + 'static,
    {
        pub fn new(store: S, outgoing_handler: O) -> Self {
            SettlementApi {
                store,
                outgoing_handler,
                account_type: PhantomData,
            }
        }

        // Helper function that returns any idempotent data that corresponds to a
        // provided idempotency key. It fails if the hash of the input that
        // generated the idempotent data does not match the hash of the provided input.
        fn check_idempotency(
            &self,
            idempotency_key: String,
            input_hash: [u8; 32],
        ) -> impl Future<Item = (StatusCode, Bytes), Error = String> {
            self.store
                .load_idempotent_data(idempotency_key.clone())
                .map_err(move |_| {
                    let error_msg = "Couldn't load idempotent data".to_owned();
                    error!("{}", error_msg);
                    error_msg
                })
                .and_then(move |ret: IdempotentData| {
                    if ret.2 == input_hash {
                        Ok((ret.0, ret.1))
                    } else {
                        Ok((StatusCode::from_u16(409).unwrap(), Bytes::from(&b"Provided idempotency key is tied to other input"[..])))
                    }
                })
        }

        fn make_idempotent_call<F>(&self, f: F, input_hash: [u8; 32], idempotency_key: Option<String>) -> impl Future<Item = Response<Bytes>, Error = Response<String>>
        where F: FnOnce() -> Box<dyn Future<Item = (StatusCode, Bytes), Error = (StatusCode, String)> + Send> {
            let store = self.store.clone();
            if let Some(idempotency_key) = idempotency_key {
                // If there an idempotency key was provided, check idempotency
                // and the key was not present or conflicting with an existing
                // key, perform the call and save the idempotent return data
                Either::A(
                    self.check_idempotency(idempotency_key.clone(), input_hash)
                    .then(move |ret: Result<(StatusCode, Bytes), String>| {
                        if let Ok(ret) = ret {
                            if ret.0.is_success() {
                                let resp = Response::builder().status(ret.0).body(ret.1).unwrap();
                                return Either::A(Either::A(ok(resp)))
                            } else {
                                let resp = Response::builder().status(ret.0).body(String::from_utf8_lossy(&ret.1).to_string()).unwrap();
                                return Either::A(Either::B(err(resp)))

                            }
                        } else {
                            Either::B(
                                f()
                                .map_err({let store = store.clone(); let idempotency_key = idempotency_key.clone(); move |ret: (StatusCode, String)| {
                                    let status_code = ret.0;
                                    let data = Bytes::from(ret.1.clone());
                                    spawn(store.save_idempotent_data(idempotency_key, input_hash, status_code, data));
                                    Response::builder().status(status_code).body(ret.1).unwrap()
                                }})
                                .and_then(move |ret: (StatusCode, Bytes)| {
                                    store.save_idempotent_data(idempotency_key, input_hash, ret.0, ret.1.clone())
                                    .map_err({let ret = ret.clone(); move |_| {
                                        Response::builder().status(ret.0).body(String::from_utf8_lossy(&ret.1).to_string()).unwrap()
                                    }}).and_then(move |_| {
                                        Ok(Response::builder().status(ret.0).body(ret.1).unwrap())
                                    })
                                })
                            )
                        }
                    })
                )
            } else {
                // otherwise just make the call w/o any idempotency saves
                Either::B(
                    f()
                    .map_err(move |ret: (StatusCode, String)| {
                        Response::builder().status(ret.0).body(ret.1).unwrap()
                    })
                    .and_then(move |ret: (StatusCode, Bytes)| {
                        Ok(Response::builder().status(ret.0).body(ret.1).unwrap())
                    })
                )
            }
        }

        #[post("/accounts/:account_id/settlements")]
        fn receive_settlement(&self, account_id: String, body: Quantity, idempotency_key: Option<String>) -> impl Future<Item = Response<Bytes>, Error = Response<String>> {
            let input = format!("{}{:?}", account_id, body);
            let input_hash = get_hash_of(input.as_ref());

            let self_clone = self.clone();
            let idempotency_key_clone = idempotency_key.clone();
            let f = move || self_clone.do_receive_settlement(account_id, body, idempotency_key_clone);
            self.make_idempotent_call(f, input_hash, idempotency_key)
        }

        fn do_receive_settlement(&self, account_id: String, body: Quantity, idempotency_key: Option<String>) -> Box<dyn Future<Item = (StatusCode, Bytes), Error = (StatusCode, String)> + Send> {
            let store = self.store.clone();
            let amount = body.amount;
            let engine_scale = body.scale;
            Box::new(result(A::AccountId::from_str(&account_id)
            .map_err(move |_err| {
                let error_msg = format!("Unable to parse account id: {}", account_id);
                error!("{}", error_msg);
                (StatusCode::from_u16(400).unwrap(), error_msg)
            }))
            .and_then({
                let store = store.clone();
                move |account_id| {
                store.get_accounts(vec![account_id])
                .map_err(move |_err| {
                    let error_msg = format!("Error getting account: {}", account_id);
                    error!("{}", error_msg);
                    (StatusCode::from_u16(404).unwrap(), error_msg)
                })
            }})
            .and_then(move |accounts| {
                let account = &accounts[0];
                if account.settlement_engine_details().is_some() {
                    Ok(account.clone())
                } else {
                    let error_msg = format!("Account {} does not have settlement engine details configured. Cannot handle incoming settlement", account.id());
                    error!("{}", error_msg);
                    Err((StatusCode::from_u16(404).unwrap(), error_msg))
                }
            })
            .and_then(move |account| {
                result(BigU256::from_dec_str(&amount))
                .map_err(move |_| {
                    let error_msg = format!("Could not convert amount: {:?}", amount);
                    error!("{}", error_msg);
                    (StatusCode::from_u16(500).unwrap(), error_msg)
                })
                .and_then(move |amount| {
                    let account_id = account.id();
                    let amount = amount.normalize_scale(ConvertDetails {
                        // scale it down so that it fits in the connector
                        from: account.asset_scale(),
                        to: engine_scale,
                    });
                    // after downscaling it, hopefully we can convert to u64 without
                    // loss of precision
                    let amount = amount.low_u64();
                    store.update_balance_for_incoming_settlement(account_id, amount, idempotency_key)
                    .map_err(move |_| {
                        let error_msg = format!("Error updating balance of account: {} for incoming settlement of amount: {}", account_id, amount);
                        error!("{}", error_msg);
                        (StatusCode::from_u16(500).unwrap(), error_msg)
                    })
                })
            }).and_then(move |_| {
                let ret = Bytes::from("Success");
                Ok((StatusCode::OK, ret))
            }))
        }

        // Gets called by our settlement engine, forwards the request outwards
        // until it reaches the peer's settlement engine. Extract is not
        // implemented for Bytes unfortunately.
       #[post("/accounts/:account_id/messages")]
       fn send_outgoing_message(&self, account_id: String, body: Vec<u8>, idempotency_key: Option<String>)-> impl Future<Item = Response<Bytes>, Error = Response<String>> {

           let input = format!("{}{:?}", account_id, body);
           let input_hash = get_hash_of(input.as_ref());

            let self_clone = self.clone();
            let f = move || self_clone.do_send_outgoing_message(account_id, body);
            self.make_idempotent_call(f, input_hash, idempotency_key)
       }

        fn do_send_outgoing_message(&self, account_id: String, body: Vec<u8>) -> Box<dyn Future<Item = (StatusCode, Bytes), Error = (StatusCode, String)> + Send> {
            let store = self.store.clone();
            let mut outgoing_handler = self.outgoing_handler.clone();
               Box::new(result(A::AccountId::from_str(&account_id)
               .map_err(move |_err| {
                   let error_msg = format!("Unable to parse account id: {}", account_id);
                   error!("{}", error_msg);
                   let status_code = StatusCode::from_u16(400).unwrap();
                   (status_code, error_msg)
               }))
               .and_then(move |account_id| {
                    store.get_accounts(vec![account_id])
                    .map_err(move |_| {
                        let error_msg = format!("Error getting account: {}", account_id);
                        error!("{}", error_msg);
                        let status_code = StatusCode::from_u16(404).unwrap();
                        (status_code, error_msg)
                    })
                })
               .and_then(|accounts| {
                   let account = &accounts[0];
                   if account.settlement_engine_details().is_some() {
                       Ok(account.clone())
                   } else {
                       let error_msg = format!("Account {} has no settlement engine details configured, cannot send a settlement engine message to that account", accounts[0].id());
                       error!("{}", error_msg);
                       Err((StatusCode::from_u16(404).unwrap(), error_msg))
                   }
               })
               .and_then(move |account| {
                   // Send the message to the peer's settlement engine.
                   // Note that we use dummy values for the `from` and `original_amount`
                   // because this `OutgoingRequest` will bypass the router and thus will not
                   // use either of these values. Including dummy values in the rare case where
                   // we do not need them seems easier than using
                   // `Option`s all over the place.
                   outgoing_handler.send_request(OutgoingRequest {
                       from: account.clone(),
                       to: account.clone(),
                       original_amount: 0,
                       prepare: PrepareBuilder {
                           destination: SE_ILP_ADDRESS.clone(),
                           amount: 0,
                           expires_at: SystemTime::now() + Duration::from_secs(30),
                           data: &body,
                           execution_condition: &PEER_PROTOCOL_CONDITION,
                       }.build()
                   })
                   .map_err(move |reject| {
                       let error_msg = format!("Error sending message to peer settlement engine. Packet rejected with code: {}, message: {}", reject.code(), str::from_utf8(reject.message()).unwrap_or_default());
                       error!("{}", error_msg);
                       (StatusCode::from_u16(502).unwrap(), error_msg)
                   })
               })
               .and_then(move |fulfill| {
                   let data = Bytes::from(fulfill.data());
                   Ok((StatusCode::OK, data))
               })
               )
        }
        pub fn serve<I>(self, incoming: I) -> impl Future<Item = (), Error = ()>
        where
            I: ConnectionStream,
            I::Item: Send + 'static,
        {
            ServiceBuilder::new()
                .resource(self)
                .serve(incoming)
        }
    }

}

fn get_hash_of(preimage: &[u8]) -> [u8; 32] {
    let mut hash = [0; 32];
    hash.copy_from_slice(digest(&SHA256, preimage).as_ref());
    hash
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures::*;
    use crate::test_helpers::*;

    // Settlement Tests
    mod settlement_tests {
        use super::*;

        #[test]
        fn settlement_ok() {
            let id = TEST_ACCOUNT_0.clone().id.to_string();
            let store = test_store(false, true);
            let api = test_api(store.clone(), false);

            let ret: Response<_> = api
                .receive_settlement(id.clone(), Quantity::new(200, 18), IDEMPOTENCY.clone())
                .wait()
                .unwrap();
            assert_eq!(ret.status(), 200);
            assert_eq!(ret.body(), "Success");

            // check that it's idempotent
            let ret: Response<_> = api
                .receive_settlement(id.clone(), Quantity::new(200, 18), IDEMPOTENCY.clone())
                .wait()
                .unwrap();
            assert_eq!(ret.status(), 200);
            assert_eq!(ret.body(), "Success");

            // fails with different account id
            let id2 = "2".to_string();
            let ret: Response<_> = api
                .receive_settlement(id2.clone(), Quantity::new(200, 18), IDEMPOTENCY.clone())
                .wait()
                .unwrap_err();
            assert_eq!(ret.status(), StatusCode::from_u16(409).unwrap());
            assert_eq!(
                ret.body(),
                "Provided idempotency key is tied to other input"
            );

            // fails with different settlement data and account id
            let ret: Response<_> = api
                .receive_settlement(id2, Quantity::new(42, 18), IDEMPOTENCY.clone())
                .wait()
                .unwrap_err();
            assert_eq!(ret.status(), StatusCode::from_u16(409).unwrap());
            assert_eq!(
                ret.body(),
                "Provided idempotency key is tied to other input"
            );

            // fails with different settlement data and same account id
            let ret: Response<_> = api
                .receive_settlement(id, Quantity::new(42, 18), IDEMPOTENCY.clone())
                .wait()
                .unwrap_err();
            assert_eq!(ret.status(), StatusCode::from_u16(409).unwrap());
            assert_eq!(
                ret.body(),
                "Provided idempotency key is tied to other input"
            );

            let s = store.clone();
            let cache = s.cache.read();
            let cached_data = cache.get(&IDEMPOTENCY.clone().unwrap()).unwrap();

            let cache_hits = s.cache_hits.read();
            assert_eq!(*cache_hits, 4);
            assert_eq!(cached_data.0, StatusCode::OK);
            assert_eq!(cached_data.1, &Bytes::from("Success"));
        }

        #[test]
        fn account_has_no_engine_configured() {
            let id = TEST_ACCOUNT_0.clone().id.to_string();
            let store = test_store(false, false);
            let api = test_api(store.clone(), false);

            let ret: Response<_> = block_on(api.receive_settlement(
                id.clone(),
                SETTLEMENT_DATA.clone(),
                IDEMPOTENCY.clone(),
            ))
            .unwrap_err();
            assert_eq!(ret.status().as_u16(), 404);
            assert_eq!(ret.body(), "Account 0 does not have settlement engine details configured. Cannot handle incoming settlement");

            // check that it's idempotent
            let ret: Response<_> =
                block_on(api.receive_settlement(id, SETTLEMENT_DATA.clone(), IDEMPOTENCY.clone()))
                    .unwrap_err();
            assert_eq!(ret.status().as_u16(), 404);
            assert_eq!(ret.body(), "Account 0 does not have settlement engine details configured. Cannot handle incoming settlement");

            let s = store.clone();
            let cache = s.cache.read();
            let cached_data = cache.get(&IDEMPOTENCY.clone().unwrap()).unwrap();

            let cache_hits = s.cache_hits.read();
            assert_eq!(*cache_hits, 1);
            assert_eq!(cached_data.0, 404);
            assert_eq!(cached_data.1, &Bytes::from("Account 0 does not have settlement engine details configured. Cannot handle incoming settlement"));
        }

        #[test]
        fn update_balance_for_incoming_settlement_fails() {
            let id = TEST_ACCOUNT_0.clone().id.to_string();
            let store = test_store(true, true);
            let api = test_api(store, false);

            let ret: Response<_> =
                block_on(api.receive_settlement(id, SETTLEMENT_DATA.clone(), IDEMPOTENCY.clone()))
                    .unwrap_err();
            assert_eq!(ret.status().as_u16(), 500);
        }

        #[test]
        fn invalid_account_id() {
            // the api is configured to take an accountId type
            // supplying an id that cannot be parsed to that type must fail
            let id = "a".to_string();
            let store = test_store(false, true);
            let api = test_api(store.clone(), false);

            let ret: Response<_> = block_on(api.receive_settlement(
                id.clone(),
                SETTLEMENT_DATA.clone(),
                IDEMPOTENCY.clone(),
            ))
            .unwrap_err();
            assert_eq!(ret.status().as_u16(), 400);
            assert_eq!(ret.body(), "Unable to parse account id: a");

            // check that it's idempotent
            let ret: Response<_> = block_on(api.receive_settlement(
                id.clone(),
                SETTLEMENT_DATA.clone(),
                IDEMPOTENCY.clone(),
            ))
            .unwrap_err();
            assert_eq!(ret.status().as_u16(), 400);
            assert_eq!(ret.body(), "Unable to parse account id: a");

            let _ret: Response<_> =
                block_on(api.receive_settlement(id, SETTLEMENT_DATA.clone(), IDEMPOTENCY.clone()))
                    .unwrap_err();

            let s = store.clone();
            let cache = s.cache.read();
            let cached_data = cache.get(&IDEMPOTENCY.clone().unwrap()).unwrap();

            let cache_hits = s.cache_hits.read();
            assert_eq!(*cache_hits, 2);
            assert_eq!(cached_data.0, 400);
            assert_eq!(cached_data.1, &Bytes::from("Unable to parse account id: a"));
        }

        #[test]
        fn account_not_in_store() {
            let id = TEST_ACCOUNT_0.clone().id.to_string();
            let store = TestStore::new(vec![], false);
            let api = test_api(store.clone(), false);

            let ret: Response<_> = block_on(api.receive_settlement(
                id.clone(),
                SETTLEMENT_DATA.clone(),
                IDEMPOTENCY.clone(),
            ))
            .unwrap_err();
            assert_eq!(ret.status().as_u16(), 404);
            assert_eq!(ret.body(), "Error getting account: 0");

            let ret: Response<_> =
                block_on(api.receive_settlement(id, SETTLEMENT_DATA.clone(), IDEMPOTENCY.clone()))
                    .unwrap_err();
            assert_eq!(ret.status().as_u16(), 404);
            assert_eq!(ret.body(), "Error getting account: 0");

            let s = store.clone();
            let cache = s.cache.read();
            let cached_data = cache.get(&IDEMPOTENCY.clone().unwrap()).unwrap();
            let cache_hits = s.cache_hits.read();
            assert_eq!(*cache_hits, 1);
            assert_eq!(cached_data.0, 404);
            assert_eq!(cached_data.1, &Bytes::from("Error getting account: 0"));
        }
    }

    mod message_tests {
        use super::*;

        #[test]
        fn message_ok() {
            let id = TEST_ACCOUNT_0.clone().id.to_string();
            let store = test_store(false, true);
            let api = test_api(store.clone(), true);

            let ret: Response<_> =
                block_on(api.send_outgoing_message(id.clone(), vec![], IDEMPOTENCY.clone()))
                    .unwrap();
            assert_eq!(ret.status(), StatusCode::OK);
            assert_eq!(ret.body(), &Bytes::from("hello!"));

            let ret: Response<_> =
                block_on(api.send_outgoing_message(id.clone(), vec![], IDEMPOTENCY.clone()))
                    .unwrap();
            assert_eq!(ret.status(), StatusCode::OK);
            assert_eq!(ret.body(), &Bytes::from("hello!"));

            // Using the same idempotency key with different arguments MUST
            // fail.
            let id2 = "1".to_string();
            let ret: Response<_> =
                block_on(api.send_outgoing_message(id2.clone(), vec![], IDEMPOTENCY.clone()))
                    .unwrap_err();
            assert_eq!(ret.status(), StatusCode::from_u16(409).unwrap());
            assert_eq!(
                ret.body(),
                "Provided idempotency key is tied to other input"
            );

            let data = vec![0, 1, 2];
            // fails with different account id and data
            let ret: Response<_> =
                block_on(api.send_outgoing_message(id2, data.clone(), IDEMPOTENCY.clone()))
                    .unwrap_err();
            assert_eq!(ret.status(), StatusCode::from_u16(409).unwrap());
            assert_eq!(
                ret.body(),
                "Provided idempotency key is tied to other input"
            );

            // fails for same account id but different data
            let ret: Response<_> =
                block_on(api.send_outgoing_message(id, data, IDEMPOTENCY.clone())).unwrap_err();
            assert_eq!(ret.status(), StatusCode::from_u16(409).unwrap());
            assert_eq!(
                ret.body(),
                "Provided idempotency key is tied to other input"
            );

            let s = store.clone();
            let cache = s.cache.read();
            let cached_data = cache.get(&IDEMPOTENCY.clone().unwrap()).unwrap();
            let cache_hits = s.cache_hits.read();
            assert_eq!(*cache_hits, 4);
            assert_eq!(cached_data.0, StatusCode::OK);
            assert_eq!(cached_data.1, &Bytes::from("hello!"));
        }

        #[test]
        fn message_gets_rejected() {
            let id = TEST_ACCOUNT_0.clone().id.to_string();
            let store = test_store(false, true);
            let api = test_api(store.clone(), false);

            let ret = block_on(api.send_outgoing_message(id.clone(), vec![], IDEMPOTENCY.clone()))
                .unwrap_err();
            assert_eq!(ret.status().as_u16(), 502);

            let ret =
                block_on(api.send_outgoing_message(id, vec![], IDEMPOTENCY.clone())).unwrap_err();
            assert_eq!(ret.status().as_u16(), 502);

            let s = store.clone();
            let cache = s.cache.read();
            let cached_data = cache.get(&IDEMPOTENCY.clone().unwrap()).unwrap();
            let cache_hits = s.cache_hits.read();
            assert_eq!(*cache_hits, 1);
            assert_eq!(cached_data.0, 502);
            assert_eq!(cached_data.1, &Bytes::from("Error sending message to peer settlement engine. Packet rejected with code: F02, message: No other outgoing handler!"));
        }

        #[test]
        fn invalid_account_id() {
            // the api is configured to take an accountId type
            // supplying an id that cannot be parsed to that type must fail
            let id = "a".to_string();
            let store = test_store(false, true);
            let api = test_api(store.clone(), true);

            let ret: Response<_> =
                block_on(api.send_outgoing_message(id.clone(), vec![], IDEMPOTENCY.clone()))
                    .unwrap_err();
            assert_eq!(ret.status().as_u16(), 400);

            let ret: Response<_> =
                block_on(api.send_outgoing_message(id.clone(), vec![], IDEMPOTENCY.clone()))
                    .unwrap_err();
            assert_eq!(ret.status().as_u16(), 400);

            let _ret: Response<_> =
                block_on(api.send_outgoing_message(id, vec![], IDEMPOTENCY.clone())).unwrap_err();
            assert_eq!(ret.status().as_u16(), 400);

            let s = store.clone();
            let cache = s.cache.read();
            let cached_data = cache.get(&IDEMPOTENCY.clone().unwrap()).unwrap();

            let cache_hits = s.cache_hits.read();
            assert_eq!(*cache_hits, 2);
            assert_eq!(cached_data.0, 400);
            assert_eq!(cached_data.1, &Bytes::from("Unable to parse account id: a"));
        }

        #[test]
        fn account_not_in_store() {
            let id = TEST_ACCOUNT_0.clone().id.to_string();
            let store = TestStore::new(vec![], false);
            let api = test_api(store.clone(), true);

            let ret: Response<_> =
                block_on(api.send_outgoing_message(id.clone(), vec![], IDEMPOTENCY.clone()))
                    .unwrap_err();
            assert_eq!(ret.status().as_u16(), 404);

            let ret: Response<_> =
                block_on(api.send_outgoing_message(id, vec![], IDEMPOTENCY.clone())).unwrap_err();
            assert_eq!(ret.status().as_u16(), 404);

            let s = store.clone();
            let cache = s.cache.read();
            let cached_data = cache.get(&IDEMPOTENCY.clone().unwrap()).unwrap();

            let cache_hits = s.cache_hits.read();
            assert_eq!(*cache_hits, 1);
            assert_eq!(cached_data.0, 404);
            assert_eq!(cached_data.1, &Bytes::from("Error getting account: 0"));
        }
    }
}
