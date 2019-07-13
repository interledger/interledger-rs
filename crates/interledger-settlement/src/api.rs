use super::{SettlementAccount, SettlementStore};
use bytes::Bytes;
use futures::{
    future::{ok, result, Either},
    Future,
};
use hyper::{Response, StatusCode};
use interledger_ildcp::IldcpAccount;
use interledger_packet::PrepareBuilder;
use interledger_service::{AccountStore, OutgoingRequest, OutgoingService};
use interledger_service_util::{Convert, ConvertDetails};
use ring::digest::{digest, SHA256};
use std::{
    marker::PhantomData,
    str::{self, FromStr},
    time::{Duration, SystemTime},
};
use tower_web::{net::ConnectionStream, ServiceBuilder};

static PEER_PROTOCOL_CONDITION: [u8; 32] = [
    102, 104, 122, 173, 248, 98, 189, 119, 108, 143, 193, 139, 142, 159, 142, 32, 8, 151, 20, 133,
    110, 226, 51, 179, 144, 42, 89, 29, 13, 95, 41, 37,
];

#[derive(Debug, Clone, Extract)]
struct SettlementData {
    amount: u64,
}

pub struct SettlementApi<S, O, A> {
    outgoing_handler: O,
    store: S,
    account_type: PhantomData<A>,
}

impl_web! {
    impl<S, O, A> SettlementApi<S, O, A>
    where
        S: SettlementStore<Account = A> + AccountStore<Account = A> + Clone + Send + Sync + 'static,
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

        fn check_idempotency(
            &self,
            idempotency_key: Option<String>,
            input_hash: [u8; 32],
        ) -> impl Future<Item = Option<(StatusCode, Bytes)>, Error = (StatusCode, String)> {
            if let Some(idempotency_key) = idempotency_key {
                Either::A(self.store.load_idempotent_data(idempotency_key.clone())
                .map_err(move |err| {
                    let err = format!("Couldn't connect to store {:?}", err);
                    error!("{}", err);
                    (StatusCode::from_u16(500).unwrap(), err)
                }).and_then(move |ret: Option<(StatusCode, Bytes, [u8; 32])>| {
                        if let Some(d) = ret {
                            if d.2 != input_hash {
                                // Stripe CONFLICT status code
                                return Err((StatusCode::from_u16(409).unwrap(), "Provided idempotency key is tied to other input".to_string()))
                            }
                            if d.0.is_success() {
                                return Ok(Some((d.0, d.1)))
                            } else {
                                return Err((d.0, String::from_utf8_lossy(&d.1).to_string()))
                            }
                        }
                        Ok(None)
                    }
                ))
            } else {
                Either::B(ok(None))
            }
        }

        #[post("/accounts/:account_id/settlement")]
        fn receive_settlement(&self, account_id: String, body: SettlementData, idempotency_key: Option<String>) -> impl Future<Item = Response<Bytes>, Error = Response<String>> {
            let amount = body.amount;
            let store = self.store.clone();

            let input = format!("{}{:?}", account_id, body);
            let input_hash = get_hash_of(input.as_ref());

            self.check_idempotency(idempotency_key.clone(), input_hash).map_err(|res| {
                Response::builder().status(res.0).body(res.1).unwrap()
            })
            .and_then(move |ret: Option<(StatusCode, Bytes)>| {
                if let Some(d) = ret {
                    return Either::A(ok(Response::builder().status(d.0).body(d.1).unwrap()));
                }
                Either::B(
                    result(A::AccountId::from_str(&account_id)
                    .map_err({
                        let store = store.clone();
                        let idempotency_key = idempotency_key.clone();
                        move |_err| {
                        let error_msg = format!("Unable to parse account id: {}", account_id);
                        error!("{}", error_msg);
                        let status_code = StatusCode::from_u16(400).unwrap();
                        let data = Bytes::from(error_msg.clone());
                        if let Some(idempotency_key) = idempotency_key {
                            store.save_idempotent_data(idempotency_key, input_hash, status_code, data);
                        }
                        Response::builder().status(400).body(error_msg).unwrap()
                    }}))
                    .and_then({
                        let store = store.clone();
                        let idempotency_key = idempotency_key.clone();
                        move |account_id| {
                        store.get_accounts(vec![account_id])
                        .map_err({
                            let store = store.clone();
                            let idempotency_key = idempotency_key.clone();
                            move |_err| {
                            let error_msg = format!("Error getting account: {}", account_id);
                            error!("{}", error_msg);

                            let status_code = StatusCode::from_u16(404).unwrap();
                            let data = Bytes::from(error_msg.clone());
                            if let Some(idempotency_key) = idempotency_key {
                                store.save_idempotent_data(idempotency_key.clone(), input_hash, status_code, data);
                            }
                            Response::builder().status(404).body(error_msg).unwrap()
                        }})
                    }})
                    .and_then({
                        let store = store.clone();
                        let idempotency_key = idempotency_key.clone();
                        move |accounts| {
                        let account = &accounts[0];
                        if let Some(settlement_engine) = account.settlement_engine_details() {
                            Ok((account.clone(), settlement_engine))
                        } else {
                            let error_msg = format!("Account {} does not have settlement engine details configured. Cannot handle incoming settlement", account.id());
                            error!("{}", error_msg);
                            if let Some(idempotency_key) = idempotency_key {
                                store.save_idempotent_data(idempotency_key, input_hash, StatusCode::from_u16(404).unwrap(), Bytes::from(error_msg.clone()));
                            }
                            Err(Response::builder().status(404).body(error_msg).unwrap())
                        }
                    }})
                    .and_then({
                        let store = store.clone();
                        let idempotency_key = idempotency_key.clone();
                        move |(account, settlement_engine)| {
                        let account_id = account.id();
                        let amount = amount.normalize_scale(ConvertDetails {
                            from: account.asset_scale(),
                            to: settlement_engine.asset_scale
                        });
                        store.update_balance_for_incoming_settlement(account_id, amount, idempotency_key)
                            .map_err(move |_| {
                                let err = format!("Error updating balance of account: {} for incoming settlement of amount: {}", account_id, amount);
                                error!("{}", err);
                                Response::builder().status(500).body(err).unwrap()
                        })
                    }}).and_then({
                        let store = store.clone();
                        let idempotency_key = idempotency_key.clone();
                        move |_| {
                        let ret = Bytes::from("Success");
                        if let Some(idempotency_key) = idempotency_key {
                            store.save_idempotent_data(idempotency_key, input_hash, StatusCode::OK, ret.clone());
                        }
                        ok(())
                        .and_then(|_| Ok(Response::builder().status(StatusCode::OK).body(ret).unwrap()))
                    }}))
                })
        }

        // Gets called by our settlement engine, forwards the request outwards
        // until it reaches the peer's settlement engine. Extract is not
        // implemented for Bytes unfortunately.
        #[post("/accounts/:account_id/messages")]
        fn send_outgoing_message(&self, account_id: String, body: Vec<u8>, idempotency_key: Option<String>)-> impl Future<Item = Response<Bytes>, Error = Response<String>> {
            let store = self.store.clone();
            let mut outgoing_handler = self.outgoing_handler.clone();

            let input = format!("{}{:?}", account_id, body);
            let input_hash = get_hash_of(input.as_ref());

            self.check_idempotency(idempotency_key.clone(), input_hash).map_err(|res| {
                Response::builder().status(res.0).body(res.1).unwrap()
            })
            .and_then(move |ret: Option<(StatusCode, Bytes)>| {
                if let Some(d) = ret {
                    return Either::A(ok(Response::builder().status(d.0).body(d.1).unwrap()));
                }
                Either::B(
                result(A::AccountId::from_str(&account_id)
                .map_err({
                    let store = store.clone();
                    let idempotency_key = idempotency_key.clone();
                    move |_err| {
                    let error_msg = format!("Unable to parse account id: {}", account_id);
                    error!("{}", error_msg);
                    let status_code = StatusCode::from_u16(400).unwrap();
                    let data = Bytes::from(error_msg.clone());
                    if let Some(idempotency_key) = idempotency_key {
                        store.save_idempotent_data(idempotency_key, input_hash, status_code, data);
                    }
                    Response::builder().status(400).body(error_msg).unwrap()
                }}))
                .and_then({
                    let store = store.clone();
                    let idempotency_key = idempotency_key.clone();
                    move |account_id|
                store.get_accounts(vec![account_id])
                .map_err({
                    move |_| {
                    let error_msg = format!("Error getting account: {}", account_id);
                    error!("{}", error_msg);
                    let status_code = StatusCode::from_u16(404).unwrap();
                    let data = Bytes::from(error_msg.clone());
                    if let Some(idempotency_key) = idempotency_key {
                        store.save_idempotent_data(idempotency_key, input_hash, status_code, data);
                    }
                    Response::builder().status(404).body(error_msg).unwrap()
                }})})
                .and_then(|accounts| {
                    let account = &accounts[0];
                    if let Some(settlement_engine) = account.settlement_engine_details() {
                        Ok((account.clone(), settlement_engine))
                    } else {
                        let err = format!("Account {} has no settlement engine details configured, cannot send a settlement engine message to that account", accounts[0].id());
                        error!("{}", err);
                        Err(Response::builder().status(404).body(err).unwrap())
                    }
                })
                .and_then({
                    let store = store.clone();
                    let idempotency_key = idempotency_key.clone();
                    move |(account, settlement_engine)| {
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
                            destination: settlement_engine.ilp_address,
                            amount: 0,
                            expires_at: SystemTime::now() + Duration::from_secs(30),
                            data: &body,
                            execution_condition: &PEER_PROTOCOL_CONDITION,
                        }.build()
                    })
                    .map_err({
                        move |reject| {
                        let error_msg = format!("Error sending message to peer settlement engine. Packet rejected with code: {}, message: {}", reject.code(), str::from_utf8(reject.message()).unwrap_or_default());
                        error!("{}", error_msg);
                        if let Some(idempotency_key) = idempotency_key {
                            store.save_idempotent_data(idempotency_key, input_hash, StatusCode::from_u16(502).unwrap(), Bytes::from(error_msg.clone()));
                        }
                        Response::builder().status(502).body(error_msg).unwrap()
                    }})
                }})
                .and_then({
                    move |fulfill| {
                    let data = Bytes::from(fulfill.data());
                    if let Some(idempotency_key) = idempotency_key {
                        store.save_idempotent_data(idempotency_key, input_hash, StatusCode::OK, data.clone());
                    }
                    ok(())
                    .and_then(|_| Ok(
                        Response::builder().status(200).body(data).unwrap()
                    ))
                }})
            )})
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
                .receive_settlement(
                    id.clone(),
                    SettlementData { amount: 200 },
                    IDEMPOTENCY.clone(),
                )
                .wait()
                .unwrap();
            assert_eq!(ret.status(), 200);
            assert_eq!(ret.body(), "Success");

            // check that it's idempotent
            let ret: Response<_> = api
                .receive_settlement(
                    id.clone(),
                    SettlementData { amount: 200 },
                    IDEMPOTENCY.clone(),
                )
                .wait()
                .unwrap();
            assert_eq!(ret.status(), 200);
            assert_eq!(ret.body(), "Success");

            // fails with different account id
            let id2 = "2".to_string();
            let ret: Response<_> = api
                .receive_settlement(
                    id2.clone(),
                    SettlementData { amount: 200 },
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
                .receive_settlement(id2, SettlementData { amount: 42 }, IDEMPOTENCY.clone())
                .wait()
                .unwrap_err();
            assert_eq!(ret.status(), StatusCode::from_u16(409).unwrap());
            assert_eq!(
                ret.body(),
                "Provided idempotency key is tied to other input"
            );

            // fails with different settlement data and same account id
            let ret: Response<_> = api
                .receive_settlement(id, SettlementData { amount: 42 }, IDEMPOTENCY.clone())
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

            let ret: Response<_> = api
                .receive_settlement(
                    id.clone(),
                    SettlementData {
                        amount: SETTLEMENT_BODY,
                    },
                    IDEMPOTENCY.clone(),
                )
                .wait()
                .unwrap_err();
            assert_eq!(ret.status().as_u16(), 404);
            assert_eq!(ret.body(), "Account 0 does not have settlement engine details configured. Cannot handle incoming settlement");

            // check that it's idempotent
            let ret: Response<_> = api
                .receive_settlement(
                    id,
                    SettlementData {
                        amount: SETTLEMENT_BODY,
                    },
                    IDEMPOTENCY.clone(),
                )
                .wait()
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

            let ret: Response<_> = api
                .receive_settlement(
                    id,
                    SettlementData {
                        amount: SETTLEMENT_BODY,
                    },
                    IDEMPOTENCY.clone(),
                )
                .wait()
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

            let ret: Response<_> = api
                .receive_settlement(
                    id.clone(),
                    SettlementData {
                        amount: SETTLEMENT_BODY,
                    },
                    IDEMPOTENCY.clone(),
                )
                .wait()
                .unwrap_err();
            assert_eq!(ret.status().as_u16(), 400);
            assert_eq!(ret.body(), "Unable to parse account id: a");

            // check that it's idempotent
            let ret: Response<_> = api
                .receive_settlement(
                    id.clone(),
                    SettlementData {
                        amount: SETTLEMENT_BODY,
                    },
                    IDEMPOTENCY.clone(),
                )
                .wait()
                .unwrap_err();
            assert_eq!(ret.status().as_u16(), 400);
            assert_eq!(ret.body(), "Unable to parse account id: a");

            let _ret: Response<_> = api
                .receive_settlement(
                    id,
                    SettlementData {
                        amount: SETTLEMENT_BODY,
                    },
                    IDEMPOTENCY.clone(),
                )
                .wait()
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

            let ret: Response<_> = api
                .receive_settlement(
                    id.clone(),
                    SettlementData {
                        amount: SETTLEMENT_BODY,
                    },
                    IDEMPOTENCY.clone(),
                )
                .wait()
                .unwrap_err();
            assert_eq!(ret.status().as_u16(), 404);
            assert_eq!(ret.body(), "Error getting account: 0");

            let ret: Response<_> = api
                .receive_settlement(
                    id,
                    SettlementData {
                        amount: SETTLEMENT_BODY,
                    },
                    IDEMPOTENCY.clone(),
                )
                .wait()
                .unwrap_err(); // Bug that returns Result::OK
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

            let ret: Response<_> = api
                .send_outgoing_message(id.clone(), vec![], IDEMPOTENCY.clone())
                .wait()
                .unwrap();
            assert_eq!(ret.status(), StatusCode::OK);
            assert_eq!(ret.body(), &Bytes::from("hello!"));

            let ret: Response<_> = api
                .send_outgoing_message(id.clone(), vec![], IDEMPOTENCY.clone())
                .wait()
                .unwrap();
            assert_eq!(ret.status(), StatusCode::OK);
            assert_eq!(ret.body(), &Bytes::from("hello!"));

            // Using the same idempotency key with different arguments MUST
            // fail.
            let id2 = "1".to_string();
            let ret: Response<_> = api
                .send_outgoing_message(id2.clone(), vec![], IDEMPOTENCY.clone())
                .wait()
                .unwrap_err();
            assert_eq!(ret.status(), StatusCode::from_u16(409).unwrap());
            assert_eq!(
                ret.body(),
                "Provided idempotency key is tied to other input"
            );

            let data = vec![0, 1, 2];
            // fails with different account id and data
            let ret: Response<_> = api
                .send_outgoing_message(id2, data.clone(), IDEMPOTENCY.clone())
                .wait()
                .unwrap_err();
            assert_eq!(ret.status(), StatusCode::from_u16(409).unwrap());
            assert_eq!(
                ret.body(),
                "Provided idempotency key is tied to other input"
            );

            // fails for same account id but different data
            let ret: Response<_> = api
                .send_outgoing_message(id, data, IDEMPOTENCY.clone())
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
            assert_eq!(cached_data.1, &Bytes::from("hello!"));
        }

        #[test]
        fn message_gets_rejected() {
            let id = TEST_ACCOUNT_0.clone().id.to_string();
            let store = test_store(false, true);
            let api = test_api(store.clone(), false);

            let ret = api
                .send_outgoing_message(id.clone(), vec![], IDEMPOTENCY.clone())
                .wait()
                .unwrap_err();
            assert_eq!(ret.status().as_u16(), 502);

            let ret = api
                .send_outgoing_message(id, vec![], IDEMPOTENCY.clone())
                .wait()
                .unwrap_err();
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

            let ret: Response<_> = api
                .send_outgoing_message(id.clone(), vec![], IDEMPOTENCY.clone())
                .wait()
                .unwrap_err();
            assert_eq!(ret.status().as_u16(), 400);

            let ret: Response<_> = api
                .send_outgoing_message(id.clone(), vec![], IDEMPOTENCY.clone())
                .wait()
                .unwrap_err();
            assert_eq!(ret.status().as_u16(), 400);

            let _ret: Response<_> = api
                .send_outgoing_message(id, vec![], IDEMPOTENCY.clone())
                .wait()
                .unwrap_err();
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

            let ret: Response<_> = api
                .send_outgoing_message(id.clone(), vec![], IDEMPOTENCY.clone())
                .wait()
                .unwrap_err();
            assert_eq!(ret.status().as_u16(), 404);

            let ret: Response<_> = api
                .send_outgoing_message(id, vec![], IDEMPOTENCY.clone())
                .wait()
                .unwrap_err();
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
