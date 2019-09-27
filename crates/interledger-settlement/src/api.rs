use super::{
    Convert, ConvertDetails, IdempotentData, IdempotentStore, LeftoversStore, Quantity,
    SettlementAccount, SettlementStore, SE_ILP_ADDRESS,
};
use bytes::Bytes;
use futures::{
    future::{err, ok, result, Either},
    Future,
};
use hyper::{Response, StatusCode};
use interledger_packet::PrepareBuilder;
use interledger_service::{Account, AccountStore, OutgoingRequest, OutgoingService};
use log::error;
use num_bigint::BigUint;
use num_traits::cast::ToPrimitive;
use num_traits::Zero;
use ring::digest::{digest, SHA256};
use serde_json::json;
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
        S: LeftoversStore<AccountId = <A as Account>::AccountId, AssetType = BigUint> + SettlementStore<Account = A> + IdempotentStore + AccountStore<Account = A> + Clone + Send + Sync + 'static,
        O: OutgoingService<A> + Clone + Send + Sync + 'static,
        A: SettlementAccount + Send + Sync + 'static,
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
        ) -> impl Future<Item = Option<(StatusCode, Bytes)>, Error = String> {
            self.store
                .load_idempotent_data(idempotency_key.clone())
                .map_err(move |_| {
                    let error_msg = "Couldn't load idempotent data".to_owned();
                    error!("{}", error_msg);
                    error_msg
                })
                .and_then(move |ret: Option<IdempotentData>| {
                    if let Some(ret) = ret {
                        if ret.2 == input_hash {
                            Ok(Some((ret.0, ret.1)))
                        } else {
                            Ok(Some((StatusCode::from_u16(409).unwrap(), Bytes::from(&b"Provided idempotency key is tied to other input"[..]))))
                        }
                    } else {
                        Ok(None)
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
                    .map_err(move |err| {
                        let status_code = StatusCode::from_u16(500).unwrap();
                        Response::builder().status(status_code).body(err).unwrap()
                    })
                    .and_then(move |ret: Option<(StatusCode, Bytes)>| {
                        if let Some(ret) = ret {
                            if ret.0.is_success() {
                                let resp = Response::builder().status(ret.0).body(ret.1).unwrap();
                                Either::A(Either::A(ok(resp)))
                            } else {
                                let resp = Response::builder().status(ret.0).body(String::from_utf8_lossy(&ret.1).to_string()).unwrap();
                                Either::A(Either::B(err(resp)))

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
            let store_clone = self.store.clone();
            let engine_amount = body.amount;
            let engine_scale = body.scale;

            // Convert to the desired data types
            let account_id = match A::AccountId::from_str(&account_id) {
                Ok(a) => a,
                Err(_) => {
                    let error_msg = format!("Unable to parse account id: {}", account_id);
                    error!("{}", error_msg);
                    return Box::new(err((StatusCode::from_u16(400).unwrap(), error_msg)))
                }
            };

            let engine_amount = match BigUint::from_str(&engine_amount) {
                Ok(a) => a,
                Err(_) => {
                    let error_msg = format!("Could not convert amount: {:?}", engine_amount);
                    error!("{}", error_msg);
                    return Box::new(err((StatusCode::from_u16(500).unwrap(), error_msg)))
                }
            };

            Box::new(
            store.get_accounts(vec![account_id])
            .map_err(move |_err| {
                let error_msg = format!("Error getting account: {}", account_id);
                error!("{}", error_msg);
                (StatusCode::from_u16(404).unwrap(), error_msg)
            })
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
                let account_id = account.id();
                let asset_scale = account.asset_scale();
                // Scale to account's scale from the engine's scale
                // If we're downscaling we might have some precision error which
                // we must save as leftovers. Upscaling is OK since we're using
                // biguint's.
                let (scaled_engine_amount, precision_loss) = scale_with_precision_loss(engine_amount, asset_scale, engine_scale);

                // This will load any leftovers (which are saved in the highest
                // so far received scale by the engine), will scale them to
                // the account's asset scale and return them. If there was any
                // precision loss due to downscaling, it will also update the
                // leftovers to the new leftovers value
                store_clone.load_uncredited_settlement_amount(account_id, asset_scale)
                .map_err(move |_err| {
                    let error_msg = format!("Error getting uncredited settlement amount for: {}", account.id());
                    error!("{}", error_msg);
                    (StatusCode::from_u16(500).unwrap(), error_msg)
                })
                .and_then(move |scaled_leftover_amount| {
                    // add the leftovers to the scaled engine amount
                    let total_amount = scaled_engine_amount.clone() + scaled_leftover_amount;
                    let engine_amount_u64 = total_amount.to_u64().unwrap_or(std::u64::MAX);

                    futures::future::join_all(vec![
                        // update the account's balance in the store
                        store.update_balance_for_incoming_settlement(account_id, engine_amount_u64, idempotency_key),
                        // save any precision loss that occurred during the
                        // scaling of the engine's amount to the account's scale
                        store.save_uncredited_settlement_amount(account_id, (precision_loss, engine_scale))
                    ])
                    .map_err(move |_| {
                        let error_msg = format!("Error updating the balance and leftovers of account: {}", account_id);
                        error!("{}", error_msg);
                        (StatusCode::from_u16(500).unwrap(), error_msg)
                    })
                    .and_then(move |_| {
                        // the connector "lies" and tells the engine that it
                        // settled the full amount. Precision loss is handled by
                        // the connector.
                        let quantity = json!(Quantity::new(total_amount, asset_scale));
                        Ok((StatusCode::OK, quantity.to_string().into()))
                    })
                })
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

pub fn scale_with_precision_loss(
    amount: BigUint,
    local_scale: u8,
    remote_scale: u8,
) -> (BigUint, BigUint) {
    // It's safe to unwrap here since BigUint's normalize_scale cannot fail.
    let scaled = amount
        .normalize_scale(ConvertDetails {
            from: remote_scale,
            to: local_scale,
        })
        .unwrap();

    if local_scale < remote_scale {
        // If we ended up downscaling, scale the value back up back,
        // and return any precision loss
        // note that `from` and `to` are reversed compared to the previous call
        let upscaled = scaled
            .normalize_scale(ConvertDetails {
                from: local_scale,
                to: remote_scale,
            })
            .unwrap();
        let precision_loss = if upscaled < amount {
            amount - upscaled
        } else {
            Zero::zero()
        };
        (scaled, precision_loss)
    } else {
        // there is no need to do anything further if we upscaled
        (scaled, Zero::zero())
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

    #[test]
    fn precision_loss() {
        assert_eq!(
            scale_with_precision_loss(BigUint::from(905u32), 9, 11),
            (BigUint::from(9u32), BigUint::from(5u32))
        );

        assert_eq!(
            scale_with_precision_loss(BigUint::from(8053u32), 9, 12),
            (BigUint::from(8u32), BigUint::from(53u32))
        );

        assert_eq!(
            scale_with_precision_loss(BigUint::from(1u32), 9, 6),
            (BigUint::from(1000u32), BigUint::from(0u32))
        );
    }

    // Settlement Tests
    mod settlement_tests {
        use super::*;

        const CONNECTOR_SCALE: u8 = 9;
        const OUR_SCALE: u8 = 11;

        #[test]
        fn settlement_ok() {
            let id = TEST_ACCOUNT_0.clone().id.to_string();
            let store = test_store(false, true);
            let api = test_api(store.clone(), false);

            // The operator accounts are configured to work with CONNECTOR_SCALE
            // = 9. When
            // we send a settlement with scale OUR_SCALE, the connector should respond
            // with 2 less 0's.
            let ret: Response<_> = api
                .receive_settlement(
                    id.clone(),
                    Quantity::new(200, OUR_SCALE),
                    IDEMPOTENCY.clone(),
                )
                .wait()
                .unwrap();
            assert_eq!(ret.status(), 200);
            let quantity: Quantity = serde_json::from_slice(ret.body()).unwrap();
            assert_eq!(quantity, Quantity::new(2, CONNECTOR_SCALE));

            // check that it's idempotent
            let ret: Response<_> = api
                .receive_settlement(
                    id.clone(),
                    Quantity::new(200, OUR_SCALE),
                    IDEMPOTENCY.clone(),
                )
                .wait()
                .unwrap();
            assert_eq!(ret.status(), 200);
            let quantity: Quantity = serde_json::from_slice(ret.body()).unwrap();
            assert_eq!(quantity, Quantity::new(2, CONNECTOR_SCALE));

            // fails with different account id
            let id2 = "2".to_string();
            let ret: Response<_> = api
                .receive_settlement(
                    id2.clone(),
                    Quantity::new(200, OUR_SCALE),
                    IDEMPOTENCY.clone(),
                )
                .wait()
                .unwrap_err();
            assert_eq!(ret.status(), StatusCode::from_u16(409).unwrap());
            assert_eq!(
                ret.body(),
                "Provided idempotency key is tied to other input"
            );

            // fails with different settlement data and account id
            let ret: Response<_> = api
                .receive_settlement(id2, Quantity::new(42, OUR_SCALE), IDEMPOTENCY.clone())
                .wait()
                .unwrap_err();
            assert_eq!(ret.status(), StatusCode::from_u16(409).unwrap());
            assert_eq!(
                ret.body(),
                "Provided idempotency key is tied to other input"
            );

            // fails with different settlement data and same account id
            let ret: Response<_> = api
                .receive_settlement(id, Quantity::new(42, OUR_SCALE), IDEMPOTENCY.clone())
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
            let quantity: Quantity = serde_json::from_slice(&cached_data.1).unwrap();
            assert_eq!(quantity, Quantity::new(2, CONNECTOR_SCALE));
        }

        #[test]
        // The connector must save the difference each time there's precision
        // loss and try to add it the amount it's being notified to settle for the next time.
        fn settlement_leftovers() {
            let id = TEST_ACCOUNT_0.clone().id.to_string();
            let store = test_store(false, true);
            let api = test_api(store.clone(), false);

            // Send 205 with scale 11, 2 decimals lost -> 0.05 leftovers
            let ret: Response<_> = api
                .receive_settlement(id.clone(), Quantity::new(205, 11), None)
                .wait()
                .unwrap();
            assert_eq!(ret.status(), 200);

            // balance should be 2
            assert_eq!(store.get_balance(TEST_ACCOUNT_0.id), 2);
            assert_eq!(
                store
                    .get_uncredited_settlement_amount(TEST_ACCOUNT_0.id)
                    .wait()
                    .unwrap(),
                (BigUint::from(5u32), 11)
            );

            // Send 855 with scale 12, 3 decimals lost -> 0.855 leftovers,
            let ret: Response<_> = api
                .receive_settlement(id.clone(), Quantity::new(855, 12), None)
                .wait()
                .unwrap();
            assert_eq!(ret.status(), 200);
            // balance should remain unchanged since the leftovers were smaller
            // than a unit's worth
            assert_eq!(store.get_balance(TEST_ACCOUNT_0.id), 2);
            // total leftover: 0.905 = 0.05 + 0.855
            assert_eq!(
                store
                    .get_uncredited_settlement_amount(TEST_ACCOUNT_0.id)
                    .wait()
                    .unwrap(),
                (BigUint::from(905u32), 12)
            );

            // send 110 with scale 11, 2 decimals lost -> 0.1 leftover
            let ret: Response<_> = api
                .receive_settlement(id.clone(), Quantity::new(110, 11), None)
                .wait()
                .unwrap();
            assert_eq!(ret.status(), 200);
            // total leftover 1.005 = 0.905 + 0.1
            // leftovers will get applied on the next settlement
            assert_eq!(store.get_balance(TEST_ACCOUNT_0.id), 3);
            assert_eq!(
                store
                    .get_uncredited_settlement_amount(TEST_ACCOUNT_0.id)
                    .wait()
                    .unwrap(),
                (BigUint::from(1005u32), 12)
            );

            // send 5 with scale 9, will consume the leftovers and increase
            // total balance by 6 while updating the rest of the leftovers
            let ret: Response<_> = api
                .receive_settlement(id.clone(), Quantity::new(5, 9), None)
                .wait()
                .unwrap();
            assert_eq!(ret.status(), 200);
            // 5 from this call + 3 from before + 1 from leftovers = 9
            assert_eq!(store.get_balance(TEST_ACCOUNT_0.id), 9);
            assert_eq!(
                store
                    .get_uncredited_settlement_amount(TEST_ACCOUNT_0.id)
                    .wait()
                    .unwrap(),
                (BigUint::from(5u32), 12)
            );

            // we send a payment with a smaller scale than the account now
            let ret: Response<_> = api
                .receive_settlement(id.clone(), Quantity::new(2, 7), None)
                .wait()
                .unwrap();
            assert_eq!(ret.status(), 200);
            assert_eq!(store.get_balance(TEST_ACCOUNT_0.id), 209);
            // leftovers are still the same
            assert_eq!(
                store
                    .get_uncredited_settlement_amount(TEST_ACCOUNT_0.id)
                    .wait()
                    .unwrap(),
                (BigUint::from(5u32), 12)
            );
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
