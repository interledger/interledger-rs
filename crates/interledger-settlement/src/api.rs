use super::{
    Convert, ConvertDetails, IdempotentData, IdempotentStore, LeftoversStore, Quantity,
    SettlementAccount, SettlementStore, SE_ILP_ADDRESS,
};
use bytes::buf::FromBuf;
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
    str::{self, FromStr},
    time::{Duration, SystemTime},
};
use tokio::executor::spawn;
use warp::{self, Filter};

static PEER_PROTOCOL_CONDITION: [u8; 32] = [
    102, 104, 122, 173, 248, 98, 189, 119, 108, 143, 193, 139, 142, 159, 142, 32, 8, 151, 20, 133,
    110, 226, 51, 179, 144, 42, 89, 29, 13, 95, 41, 37,
];

pub fn create_settlements_filter<S, O, A>(
    store: S,
    outgoing_handler: O,
) -> warp::filters::BoxedFilter<(impl warp::Reply,)>
where
    S: LeftoversStore<AccountId = <A as Account>::AccountId, AssetType = BigUint>
        + SettlementStore<Account = A>
        + IdempotentStore
        + AccountStore<Account = A>
        + Clone
        + Send
        + Sync
        + 'static,
    O: OutgoingService<A> + Clone + Send + Sync + 'static,
    A: SettlementAccount + Send + Sync + 'static,
{
    let with_store = warp::any().map(move || store.clone()).boxed();
    let idempotency = warp::header::optional::<String>("idempotency-key");
    let account_id = warp::path("accounts").and(warp::path::param2::<String>()); // account_id

    // POST /accounts/:account_id/settlements (optional idempotency-key header)
    // Body is a Quantity object
    let settlement_endpoint = account_id.and(warp::path("settlements"));
    let settlements = warp::post2()
        .and(settlement_endpoint)
        .and(warp::path::end())
        .and(idempotency.clone())
        .and(warp::body::json())
        .and(with_store.clone())
        .and_then(
            move |id: String, idempotency_key: Option<String>, quantity: Quantity, store: S| {
                let input = format!("{}{:?}", id, quantity);
                let input_hash = get_hash_of(input.as_ref());

                let idempotency_key_clone = idempotency_key.clone();
                let store_clone = store.clone();
                let f =
                    move || do_receive_settlement(store_clone, id, quantity, idempotency_key_clone);
                make_idempotent_call(store, f, input_hash, idempotency_key)
                    // TODO Replace with responses
                    .map_err(move |(_status_code, error_msg)| warp::reject::custom(error_msg))
                    .and_then(move |(_status_code, message)| {
                        // TODO Replace with responses
                        match serde_json::from_slice::<Quantity>(&message) {
                            Ok(quantity) => Either::A(ok(warp::reply::json(&quantity))),
                            Err(_) => Either::B(err(warp::reject::custom(
                                "could not convert to quantity",
                            ))),
                        }
                    })
            },
        );

    // POST /accounts/:account_id/messages (optional idempotency-key header)
    // Body is a Vec<u8> object
    let with_outgoing_handler = warp::any().map(move || outgoing_handler.clone()).boxed();
    let messages_endpoint = account_id.and(warp::path("messages"));
    let messages = warp::post2()
        .and(messages_endpoint)
        .and(warp::path::end())
        .and(idempotency.clone())
        .and(warp::body::concat())
        .and(with_store.clone())
        .and(with_outgoing_handler.clone())
        .and_then(
            move |id: String,
                  idempotency_key: Option<String>,
                  body: warp::body::FullBody,
                  store: S,
                  outgoing_handler: O| {
                // Gets called by our settlement engine, forwards the request outwards
                // until it reaches the peer's settlement engine.
                let message = Vec::from_buf(body);
                let input = format!("{}{:?}", id, message);
                let input_hash = get_hash_of(input.as_ref());

                let store_clone = store.clone();
                let f =
                    move || do_send_outgoing_message(store_clone, outgoing_handler, id, message);
                make_idempotent_call(store, f, input_hash, idempotency_key)
                    // TODO Replace with error case with response
                    .map_err(move |(_status_code, error_msg)| warp::reject::custom(error_msg))
                    .and_then(move |(status_code, message)| {
                        Ok(Response::builder()
                            .status(status_code)
                            .body(message)
                            .unwrap())
                    })
            },
        );

    settlements
        // TODO: Figure out how to convert these rejections to responses
        // .recover(|err: warp::Rejection| {
        //     Ok(err.into_response())
        // })
        .or(messages)
        .boxed()
}

// Helper function that returns any idempotent data that corresponds to a
// provided idempotency key. It fails if the hash of the input that
// generated the idempotent data does not match the hash of the provided input.
fn check_idempotency<S, A>(
    store: S,
    idempotency_key: String,
    input_hash: [u8; 32],
) -> impl Future<Item = Option<(StatusCode, Bytes)>, Error = String>
where
    S: LeftoversStore<AccountId = <A as Account>::AccountId, AssetType = BigUint>
        + SettlementStore<Account = A>
        + IdempotentStore
        + AccountStore<Account = A>
        + Clone
        + Send
        + Sync
        + 'static,
    A: SettlementAccount + Send + Sync + 'static,
{
    store
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
                    Ok(Some((
                        StatusCode::from_u16(409).unwrap(),
                        Bytes::from("Provided idempotency key is tied to other input"),
                    )))
                }
            } else {
                Ok(None)
            }
        })
}

fn make_idempotent_call<S, A, F>(
    store: S,
    f: F,
    input_hash: [u8; 32],
    idempotency_key: Option<String>,
) -> impl Future<Item = (StatusCode, Bytes), Error = (StatusCode, String)>
where
    F: FnOnce() -> Box<dyn Future<Item = (StatusCode, Bytes), Error = (StatusCode, String)> + Send>,
    S: LeftoversStore<AccountId = <A as Account>::AccountId, AssetType = BigUint>
        + SettlementStore<Account = A>
        + IdempotentStore
        + AccountStore<Account = A>
        + Clone
        + Send
        + Sync
        + 'static,
    A: SettlementAccount + Send + Sync + 'static,
{
    if let Some(idempotency_key) = idempotency_key {
        // If there an idempotency key was provided, check idempotency
        // and the key was not present or conflicting with an existing
        // key, perform the call and save the idempotent return data
        Either::A(
            check_idempotency(store.clone(), idempotency_key.clone(), input_hash)
                .map_err(move |err| {
                    let status_code = StatusCode::from_u16(500).unwrap();
                    (status_code, err)
                })
                .and_then(move |ret: Option<(StatusCode, Bytes)>| {
                    if let Some(ret) = ret {
                        if ret.0.is_success() {
                            Either::A(Either::A(ok((ret.0, ret.1))))
                        } else {
                            Either::A(Either::B(err((
                                ret.0,
                                String::from_utf8_lossy(&ret.1).to_string(),
                            ))))
                        }
                    } else {
                        Either::B(
                            f().map_err({
                                let store = store.clone();
                                let idempotency_key = idempotency_key.clone();
                                move |ret: (StatusCode, String)| {
                                    let status_code = ret.0;
                                    let data = Bytes::from(ret.1.clone());
                                    spawn(store.save_idempotent_data(
                                        idempotency_key,
                                        input_hash,
                                        status_code,
                                        data,
                                    ));
                                    (status_code, ret.1)
                                }
                            })
                            .and_then(
                                move |ret: (StatusCode, Bytes)| {
                                    store
                                        .save_idempotent_data(
                                            idempotency_key,
                                            input_hash,
                                            ret.0,
                                            ret.1.clone(),
                                        )
                                        .map_err({
                                            let ret = ret.clone();
                                            move |_| {
                                                (ret.0, String::from_utf8_lossy(&ret.1).to_string())
                                            }
                                        })
                                        .and_then(move |_| Ok((ret.0, ret.1)))
                                },
                            ),
                        )
                    }
                }),
        )
    } else {
        // otherwise just make the call w/o any idempotency saves
        Either::B(
            f().map_err(move |ret: (StatusCode, String)| (ret.0, ret.1))
                .and_then(move |ret: (StatusCode, Bytes)| Ok((ret.0, ret.1))),
        )
    }
}

fn do_receive_settlement<S, A>(
    store: S,
    account_id: String,
    body: Quantity,
    idempotency_key: Option<String>,
) -> Box<dyn Future<Item = (StatusCode, Bytes), Error = (StatusCode, String)> + Send>
where
    S: LeftoversStore<AccountId = <A as Account>::AccountId, AssetType = BigUint>
        + SettlementStore<Account = A>
        + IdempotentStore
        + AccountStore<Account = A>
        + Clone
        + Send
        + Sync
        + 'static,
    A: SettlementAccount + Send + Sync + 'static,
{
    let store_clone = store.clone();
    let engine_amount = body.amount;
    let engine_scale = body.scale;

    // Convert to the desired data types
    let account_id = match A::AccountId::from_str(&account_id) {
        Ok(a) => a,
        Err(_) => {
            let error_msg = format!("Unable to parse account id: {}", account_id);
            error!("{}", error_msg);
            return Box::new(err((StatusCode::from_u16(400).unwrap(), error_msg)));
        }
    };

    let engine_amount = match BigUint::from_str(&engine_amount) {
        Ok(a) => a,
        Err(_) => {
            let error_msg = format!("Could not convert amount: {:?}", engine_amount);
            error!("{}", error_msg);
            return Box::new(err((StatusCode::from_u16(500).unwrap(), error_msg)));
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

fn do_send_outgoing_message<S, O, A>(
    store: S,
    mut outgoing_handler: O,
    account_id: String,
    body: Vec<u8>,
) -> Box<dyn Future<Item = (StatusCode, Bytes), Error = (StatusCode, String)> + Send>
where
    S: LeftoversStore<AccountId = <A as Account>::AccountId, AssetType = BigUint>
        + SettlementStore<Account = A>
        + IdempotentStore
        + AccountStore<Account = A>
        + Clone
        + Send
        + Sync
        + 'static,
    O: OutgoingService<A> + Clone + Send + Sync + 'static,
    A: SettlementAccount + Send + Sync + 'static,
{
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
            }))
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
        use warp::Reply;

        const CONNECTOR_SCALE: u8 = 9;
        const OUR_SCALE: u8 = 11;

        fn settlement_call<F>(
            api: &F,
            id: &str,
            amount: u64,
            scale: u8,
            idempotency_key: Option<&str>,
        ) -> Response<Bytes>
        where
            F: warp::Filter + 'static,
            F::Extract: warp::Reply,
        {
            let mut response = warp::test::request()
                .method("POST")
                .path(&format!("/accounts/{}/settlements", id.clone()))
                .body(json!(Quantity::new(amount.to_string(), scale)).to_string());

            if let Some(idempotency_key) = idempotency_key {
                response = response.header("Idempotency-Key", idempotency_key);
            }
            response.reply(api)
        }

        #[test]
        fn settlement_ok() {
            let id = TEST_ACCOUNT_0.clone().id.to_string();
            let store = test_store(false, true);
            let api = test_api(store.clone(), false);

            // The operator accounts are configured to work with CONNECTOR_SCALE
            // = 9. When
            // we send a settlement with scale OUR_SCALE, the connector should respond
            // with 2 less 0's.
            let response = settlement_call(&api, &id, 200, OUR_SCALE, Some(IDEMPOTENCY));
            let quantity = serde_json::from_slice::<Quantity>(&response.body()).unwrap();
            assert_eq!(quantity, Quantity::new(2, CONNECTOR_SCALE));

            // check that it's idempotent
            let response = settlement_call(&api, &id, 200, OUR_SCALE, Some(IDEMPOTENCY));
            let quantity = serde_json::from_slice::<Quantity>(&response.body()).unwrap();
            assert_eq!(quantity, Quantity::new(2, CONNECTOR_SCALE));

            // fails with different account id
            let response = settlement_call(&api, "2", 200, OUR_SCALE, Some(IDEMPOTENCY));
            assert_eq!(
                *response.body(),
                Bytes::from("Unhandled rejection: Provided idempotency key is tied to other input")
            );

            // fails with different settlement data and account id
            let response = settlement_call(&api, "2", 42, OUR_SCALE, Some(IDEMPOTENCY));
            assert_eq!(
                *response.body(),
                Bytes::from("Unhandled rejection: Provided idempotency key is tied to other input")
            );

            // fails with different settlement data and same account id
            let response = settlement_call(&api, &id, 42, OUR_SCALE, Some(IDEMPOTENCY));
            assert_eq!(
                *response.body(),
                Bytes::from("Unhandled rejection: Provided idempotency key is tied to other input")
            );

            // works without idempotency key
            let response = settlement_call(&api, &id, 400, OUR_SCALE, None);
            let quantity = serde_json::from_slice::<Quantity>(&response.body()).unwrap();
            assert_eq!(quantity, Quantity::new(4, CONNECTOR_SCALE));

            let s = store.clone();
            let cache = s.cache.read();
            let cached_data = cache.get(&IDEMPOTENCY.to_owned()).unwrap();

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
            let response = settlement_call(&api, &id, 205, 11, None);
            let quantity = serde_json::from_slice::<Quantity>(&response.body()).unwrap();
            assert_eq!(quantity, Quantity::new(2, 9)); // there was a precision loss

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
            let response = settlement_call(&api, &id, 855, 12, None);
            let quantity = serde_json::from_slice::<Quantity>(&response.body()).unwrap();
            assert_eq!(quantity, Quantity::new(0, 9)); // there was full precision loss
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
            let response = settlement_call(&api, &id, 110, 11, None);
            let quantity = serde_json::from_slice::<Quantity>(&response.body()).unwrap();
            assert_eq!(quantity, Quantity::new(1, 9)); // there was some precision loss
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
            let response = settlement_call(&api, &id, 5, 9, None);
            let quantity = serde_json::from_slice::<Quantity>(&response.body()).unwrap();
            assert_eq!(quantity, Quantity::new(6, 9)); // there was no precision loss
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
            let response = settlement_call(&api, &id, 2, 7, None);
            let quantity = serde_json::from_slice::<Quantity>(&response.body()).unwrap();
            assert_eq!(quantity, Quantity::new(200, 9)); // there was no precision loss
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

            let response = settlement_call(&api, &id, 100, 18, Some(IDEMPOTENCY.clone()));
            assert_eq!(response.body(), "Unhandled rejection: Account 0 does not have settlement engine details configured. Cannot handle incoming settlement");

            // check that it's idempotent
            let response = settlement_call(&api, &id, 100, 18, Some(IDEMPOTENCY.clone()));
            assert_eq!(response.body(), "Unhandled rejection: Account 0 does not have settlement engine details configured. Cannot handle incoming settlement");

            let s = store.clone();
            let cache = s.cache.read();
            let cached_data = cache.get(IDEMPOTENCY).unwrap();

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
            let response = settlement_call(&api, &id, 100, 18, None);
            assert_eq!(response.status().as_u16(), 500);
        }

        #[test]
        fn invalid_account_id() {
            // the api is configured to take an accountId type
            // supplying an id that cannot be parsed to that type must fail
            let id = "a".to_string();
            let store = test_store(false, true);
            let api = test_api(store.clone(), false);

            let ret = settlement_call(&api, &id, 100, 18, Some(IDEMPOTENCY.clone()));
            // assert_eq!(ret.status().as_u16(), 400);
            assert_eq!(
                ret.body(),
                "Unhandled rejection: Unable to parse account id: a"
            );

            // check that it's idempotent
            let ret = settlement_call(&api, &id, 100, 18, Some(IDEMPOTENCY.clone()));
            // assert_eq!(ret.status().as_u16(), 400);
            assert_eq!(
                ret.body(),
                "Unhandled rejection: Unable to parse account id: a"
            );

            let _ret = settlement_call(&api, &id, 100, 18, Some(IDEMPOTENCY.clone()));

            let s = store.clone();
            let cache = s.cache.read();
            let cached_data = cache.get(IDEMPOTENCY).unwrap();

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

            let ret = settlement_call(&api, &id, 100, 18, Some(IDEMPOTENCY.clone()));
            // assert_eq!(ret.status().as_u16(), 404);
            assert_eq!(ret.body(), "Unhandled rejection: Error getting account: 0");

            let ret = settlement_call(&api, &id, 100, 18, Some(IDEMPOTENCY.clone()));
            // assert_eq!(ret.status().as_u16(), 404);
            assert_eq!(ret.body(), "Unhandled rejection: Error getting account: 0");

            let s = store.clone();
            let cache = s.cache.read();
            let cached_data = cache.get(IDEMPOTENCY).unwrap();
            let cache_hits = s.cache_hits.read();
            assert_eq!(*cache_hits, 1);
            // assert_eq!(cached_data.0, 404);
            assert_eq!(cached_data.1, &Bytes::from("Error getting account: 0"));
        }
    }

    mod message_tests {
        use super::*;

        fn messages_call<F>(
            api: &F,
            id: &str,
            message: &[u8],
            idempotency_key: Option<&str>,
        ) -> Response<Bytes>
        where
            F: warp::Filter + 'static,
            F::Extract: warp::Reply,
        {
            let mut response = warp::test::request()
                .method("POST")
                .path(&format!("/accounts/{}/messages", id.clone()))
                .body(message);

            if let Some(idempotency_key) = idempotency_key {
                response = response.header("Idempotency-Key", idempotency_key);
            }
            response.reply(api)
        }

        #[test]
        fn message_ok() {
            let id = TEST_ACCOUNT_0.clone().id.to_string();
            let store = test_store(false, true);
            let api = test_api(store.clone(), true);

            let ret = messages_call(&api, &id, &[], Some(IDEMPOTENCY));
            assert_eq!(ret.status(), StatusCode::OK);
            assert_eq!(ret.body(), &Bytes::from("hello!"));

            let ret = messages_call(&api, &id, &[], Some(IDEMPOTENCY));
            assert_eq!(ret.status(), StatusCode::OK);
            assert_eq!(ret.body(), &Bytes::from("hello!"));

            // Using the same idempotency key with different arguments MUST
            // fail.
            let ret = messages_call(&api, "1", &[], Some(IDEMPOTENCY));
            // assert_eq!(ret.status(), StatusCode::from_u16(409).unwrap());
            assert_eq!(
                ret.body(),
                "Unhandled rejection: Provided idempotency key is tied to other input"
            );

            let data = [0, 1, 2];
            // fails with different account id and data
            let ret = messages_call(&api, "1", &data[..], Some(IDEMPOTENCY));
            // assert_eq!(ret.status(), StatusCode::from_u16(409).unwrap());
            assert_eq!(
                ret.body(),
                "Unhandled rejection: Provided idempotency key is tied to other input"
            );

            // fails for same account id but different data
            let ret = messages_call(&api, &id, &data[..], Some(IDEMPOTENCY));
            // assert_eq!(ret.status(), StatusCode::from_u16(409).unwrap());
            assert_eq!(
                ret.body(),
                "Unhandled rejection: Provided idempotency key is tied to other input"
            );

            let s = store.clone();
            let cache = s.cache.read();
            let cached_data = cache.get(IDEMPOTENCY).unwrap();
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

            let ret = messages_call(&api, &id, &[], Some(IDEMPOTENCY));
            // assert_eq!(ret.status().as_u16(), 502);
            assert_eq!(ret.body(), "Unhandled rejection: Error sending message to peer settlement engine. Packet rejected with code: F02, message: No other outgoing handler!");

            let ret = messages_call(&api, &id, &[], Some(IDEMPOTENCY));
            // assert_eq!(ret.status().as_u16(), 502);
            assert_eq!(ret.body(), "Unhandled rejection: Error sending message to peer settlement engine. Packet rejected with code: F02, message: No other outgoing handler!");

            let s = store.clone();
            let cache = s.cache.read();
            let cached_data = cache.get(IDEMPOTENCY).unwrap();
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

            let ret = messages_call(&api, &id, &[], Some(IDEMPOTENCY));
            assert_eq!(
                ret.body(),
                &Bytes::from("Unhandled rejection: Unable to parse account id: a")
            );
            // assert_eq!(ret.status().as_u16(), 400);

            let ret = messages_call(&api, &id, &[], Some(IDEMPOTENCY));
            // assert_eq!(ret.status().as_u16(), 400);

            let ret = messages_call(&api, &id, &[], Some(IDEMPOTENCY));
            // assert_eq!(ret.status().as_u16(), 400);

            let s = store.clone();
            let cache = s.cache.read();
            let cached_data = cache.get(IDEMPOTENCY).unwrap();

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

            let ret = messages_call(&api, &id, &[], Some(IDEMPOTENCY));
            // assert_eq!(ret.status().as_u16(), 404);

            let ret = messages_call(&api, &id, &[], Some(IDEMPOTENCY));
            // assert_eq!(ret.status().as_u16(), 404);

            let s = store.clone();
            let cache = s.cache.read();
            let cached_data = cache.get(IDEMPOTENCY).unwrap();

            let cache_hits = s.cache_hits.read();
            assert_eq!(*cache_hits, 1);
            assert_eq!(cached_data.0, 404);
            assert_eq!(cached_data.1, &Bytes::from("Error getting account: 0"));
        }
    }
}
