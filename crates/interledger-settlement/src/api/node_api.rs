use crate::core::{
    get_hash_of,
    idempotency::*,
    scale_with_precision_loss,
    types::{
        ApiResponse, ApiResult, LeftoversStore, Quantity, SettlementAccount, SettlementStore,
        CONVERSION_ERROR_TYPE, SE_ILP_ADDRESS,
    },
};
use bytes::Bytes;
use futures::future::Either;
use futures::TryFutureExt;
use hyper::{Response, StatusCode};
use interledger_errors::*;
use interledger_packet::PrepareBuilder;
use interledger_service::{Account, AccountStore, OutgoingRequest, OutgoingService};
use num_bigint::BigUint;
use num_traits::cast::ToPrimitive;
use std::{
    str::{self, FromStr},
    time::{Duration, SystemTime},
};
use tracing::error;
use uuid::Uuid;
use warp::{self, reject::Rejection, Filter};

/// Prepare packet's execution condition as defined in the [RFC](https://interledger.org/rfcs/0038-settlement-engines/#exchanging-messages)
static PEER_PROTOCOL_CONDITION: [u8; 32] = [
    102, 104, 122, 173, 248, 98, 189, 119, 108, 143, 193, 139, 142, 159, 142, 32, 8, 151, 20, 133,
    110, 226, 51, 179, 144, 42, 89, 29, 13, 95, 41, 37,
];

/// Makes an idempotent call to [`do_receive_settlement`](./fn.do_receive_settlement.html)
/// Returns Status Code 201
async fn receive_settlement<S, A>(
    account_id: String,
    idempotency_key: Option<String>,
    quantity: Quantity,
    store: S,
) -> Result<impl warp::Reply, Rejection>
where
    S: LeftoversStore<AccountId = Uuid, AssetType = BigUint>
        + SettlementStore<Account = A>
        + IdempotentStore
        + AccountStore<Account = A>
        + Clone
        + Send
        + Sync
        + 'static,
    A: SettlementAccount + Account + Send + Sync + 'static,
{
    let input = format!("{}{:?}", account_id, quantity);
    let input_hash = get_hash_of(input.as_ref());

    let idempotency_key_clone = idempotency_key.clone();
    let store_clone = store.clone();
    let (status_code, message) = make_idempotent_call(
        store,
        do_receive_settlement(store_clone, account_id, quantity, idempotency_key_clone),
        input_hash,
        idempotency_key,
        StatusCode::CREATED,
        "RECEIVED".into(),
    )
    .await?;
    Ok(Response::builder()
        .status(status_code)
        .body(message)
        .unwrap())
}

/// Makes an idempotent call to [`do_send_outgoing_message`](./fn.do_send_outgoing_message.html)
/// Returns Status Code 201
async fn send_message<S, A, O>(
    account_id: String,
    idempotency_key: Option<String>,
    message: Bytes,
    store: S,
    outgoing_handler: O,
) -> Result<impl warp::Reply, Rejection>
where
    S: LeftoversStore<AccountId = Uuid, AssetType = BigUint>
        + SettlementStore<Account = A>
        + IdempotentStore
        + AccountStore<Account = A>
        + Clone
        + Send
        + Sync
        + 'static,
    O: OutgoingService<A> + Clone + Send + Sync + 'static,
    A: SettlementAccount + Account + Send + Sync + 'static,
{
    let input = format!("{}{:?}", account_id, message);
    let input_hash = get_hash_of(input.as_ref());

    let store_clone = store.clone();
    let (status_code, message) = make_idempotent_call(
        store,
        do_send_outgoing_message(store_clone, outgoing_handler, account_id, message.to_vec()),
        input_hash,
        idempotency_key,
        StatusCode::CREATED,
        "SENT".into(),
    )
    .await?;
    Ok(Response::builder()
        .status(status_code)
        .body(message)
        .unwrap())
}

/// Returns a Node Settlement filter which exposes a Warp-compatible
/// idempotent API which
/// 1. receives messages about incoming settlements from the engine
/// 1. sends messages from the connector's engine to the peer's
///    message service which are sent to the peer's engine
pub fn create_settlements_filter<S, O, A>(
    store: S,
    outgoing_handler: O,
) -> warp::filters::BoxedFilter<(impl warp::Reply,)>
where
    S: LeftoversStore<AccountId = Uuid, AssetType = BigUint>
        + SettlementStore<Account = A>
        + IdempotentStore
        + AccountStore<Account = A>
        + Clone
        + Send
        + Sync
        + 'static,
    O: OutgoingService<A> + Clone + Send + Sync + 'static,
    A: SettlementAccount + Account + Send + Sync + 'static,
{
    let with_store = warp::any().map(move || store.clone()).boxed();
    let idempotency = warp::header::optional::<String>("idempotency-key");
    let account_id_filter = warp::path("accounts").and(warp::path::param::<String>()); // account_id

    // POST /accounts/:account_id/settlements (optional idempotency-key header)
    // Body is a Quantity object
    let settlement_endpoint = account_id_filter.and(warp::path("settlements"));
    let settlements = warp::post()
        .and(settlement_endpoint)
        .and(warp::path::end())
        .and(idempotency)
        .and(warp::body::json())
        .and(with_store.clone())
        .and_then(receive_settlement);

    // POST /accounts/:account_id/messages (optional idempotency-key header)
    // Body is a Vec<u8> object
    let with_outgoing_handler = warp::any().map(move || outgoing_handler.clone()).boxed();
    let messages_endpoint = account_id_filter.and(warp::path("messages"));
    let messages = warp::post()
        .and(messages_endpoint)
        .and(warp::path::end())
        .and(idempotency)
        .and(warp::body::bytes())
        .and(with_store)
        .and(with_outgoing_handler)
        .and_then(send_message);

    settlements
        .or(messages)
        .recover(default_rejection_handler)
        .boxed()
}

/// Receives a settlement message from the connector's engine, proceeds to scale it to the
/// asset scale which corresponds to the account, and finally increases the account's balance
/// by the processed amount. This implements the main functionality by which an account's credit
/// is repaid, allowing them to send out more payments
async fn do_receive_settlement<S, A>(
    store: S,
    account_id: String,
    body: Quantity,
    idempotency_key: Option<String>,
) -> ApiResult
where
    S: LeftoversStore<AccountId = Uuid, AssetType = BigUint>
        + SettlementStore<Account = A>
        + IdempotentStore
        + AccountStore<Account = A>
        + Clone
        + Send
        + Sync
        + 'static,
    A: SettlementAccount + Account + Send + Sync + 'static,
{
    let store_clone = store.clone();
    let engine_amount = body.amount;
    let engine_scale = body.scale;

    // Convert to the desired data types
    let account_id = Uuid::from_str(&account_id).map_err(move |_| {
        let err = ApiError::invalid_account_id(Some(&account_id));
        error!("{}", err);
        err
    })?;

    let engine_amount = BigUint::from_str(&engine_amount).map_err(|_| {
        let error_msg = format!("Could not convert amount: {:?}", engine_amount);
        error!("{}", error_msg);
        ApiError::from_api_error_type(&CONVERSION_ERROR_TYPE).detail(error_msg)
    })?;

    let accounts = store
        .get_accounts(vec![account_id])
        .map_err(move |_| {
            let err = ApiError::account_not_found()
                .detail(format!("Account {} was not found", account_id));
            error!("{}", err);
            err
        })
        .await?;

    let account = &accounts[0];
    if account.settlement_engine_details().is_none() {
        let err = ApiError::account_not_found().detail(format!("Account {} has no settlement engine details configured, cannot send a settlement engine message to that account", accounts[0].id()));
        error!("{}", err);
        return Err(err);
    }

    let account_id = account.id();
    let asset_scale = account.asset_scale();
    // Scale to account's scale from the engine's scale
    // If we're downscaling we might have some precision error which
    // we must save as leftovers. Upscaling is OK since we're using
    // biguint's.
    let (scaled_engine_amount, precision_loss) =
        scale_with_precision_loss(engine_amount, asset_scale, engine_scale);

    // This will load any leftovers (which are saved in the highest
    // so far received scale by the engine), will scale them to
    // the account's asset scale and return them. If there was any
    // precision loss due to downscaling, it will also update the
    // leftovers to the new leftovers value
    let scaled_leftover_amount = store_clone
        .load_uncredited_settlement_amount(account_id, asset_scale)
        .map_err(move |_err| {
            let error_msg = format!(
                "Error getting uncredited settlement amount for: {}",
                account.id()
            );
            error!("{}", error_msg);
            let error_type = ApiErrorType {
                r#type: &ProblemType::Default,
                status: StatusCode::INTERNAL_SERVER_ERROR,
                title: "Load uncredited settlement amount error",
            };
            ApiError::from_api_error_type(&error_type).detail(error_msg)
        })
        .await?;

    // add the leftovers to the scaled engine amount
    let total_amount = scaled_engine_amount.clone() + scaled_leftover_amount;
    let engine_amount_u64 = total_amount.to_u64().unwrap_or(std::u64::MAX);

    let ret = futures::future::join_all(vec![
        // update the account's balance in the store
        Either::Left(store.update_balance_for_incoming_settlement(
            account_id,
            engine_amount_u64,
            idempotency_key,
        )),
        // save any precision loss that occurred during the
        // scaling of the engine's amount to the account's scale
        Either::Right(
            store
                .save_uncredited_settlement_amount(account_id, (precision_loss, engine_scale))
                .map_err(SettlementStoreError::from),
        ),
    ])
    .await;

    // if any of the futures errored, then we should propagate that
    if ret.iter().any(|r| r.is_err()) {
        let error_msg = format!(
            "Error updating the balance and leftovers of account: {}",
            account_id
        );
        error!("{}", error_msg);
        let error_type = ApiErrorType {
            r#type: &ProblemType::Default,
            status: StatusCode::INTERNAL_SERVER_ERROR,
            title: "Balance update error",
        };
        return Err(ApiError::from_api_error_type(&error_type).detail(error_msg));
    }

    Ok(ApiResponse::Default)
}

/// Sends a messages via the provided `outgoing_handler` with the `peer.settle`
/// ILP Address as ultimate destination. This messages should get caught by the
/// peer's message service, get forwarded to their engine, and then the response
/// should be communicated back via a Fulfill or Reject packet
async fn do_send_outgoing_message<S, O, A>(
    store: S,
    outgoing_handler: O,
    account_id: String,
    body: Vec<u8>,
) -> ApiResult
where
    S: LeftoversStore<AssetType = BigUint>
        + SettlementStore<Account = A>
        + IdempotentStore
        + AccountStore<Account = A>
        + Clone
        + Send
        + Sync,
    O: OutgoingService<A> + Clone + Send + Sync,
    A: SettlementAccount + Account + Send + Sync,
{
    let account_id = Uuid::from_str(&account_id).map_err(move |_| {
        let err = ApiError::invalid_account_id(Some(&account_id));
        error!("{}", err);
        err
    })?;
    let accounts = store
        .get_accounts(vec![account_id])
        .map_err(move |_| {
            let err = ApiError::account_not_found()
                .detail(format!("Account {} was not found", account_id));
            error!("{}", err);
            err
        })
        .await?;

    let account = &accounts[0];
    if account.settlement_engine_details().is_none() {
        let err = ApiError::account_not_found().detail(format!("Account {} has no settlement engine details configured, cannot send a settlement engine message to that account", accounts[0].id()));
        error!("{}", err);
        return Err(err);
    }

    // Send the message to the peer's settlement engine.
    // Note that we use dummy values for the `from` and `original_amount`
    // because this `OutgoingRequest` will bypass the router and thus will not
    // use either of these values. Including dummy values in the rare case where
    // we do not need them seems easier than using
    // `Option`s all over the place.
    let packet = {
        let mut handler = outgoing_handler.clone();
        handler
            .send_request(OutgoingRequest {
                from: account.clone(),
                to: account.clone(),
                original_amount: 0,
                prepare: PrepareBuilder {
                    destination: SE_ILP_ADDRESS.clone(),
                    amount: 0,
                    expires_at: SystemTime::now() + Duration::from_secs(30),
                    data: &body,
                    execution_condition: &PEER_PROTOCOL_CONDITION,
                }
                .build(),
            })
            .await
    };

    match packet {
        Ok(fulfill) => {
            // TODO: Can we avoid copying here?
            let data = Bytes::copy_from_slice(fulfill.data());
            Ok(ApiResponse::Data(data))
        }
        Err(reject) => {
            let error_msg = format!("Error sending message to peer settlement engine. Packet rejected with code: {}, message: {}", reject.code(), str::from_utf8(reject.message()).unwrap_or_default());
            let error_type = ApiErrorType {
                r#type: &ProblemType::Default,
                status: StatusCode::BAD_GATEWAY,
                title: "Error sending message to peer engine",
            };
            error!("{}", error_msg);
            Err(ApiError::from_api_error_type(&error_type).detail(error_msg))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::fixtures::*;
    use crate::api::test_helpers::*;
    use serde_json::Value;

    fn check_error_status_and_message(response: Response<Bytes>, status_code: u16, message: &str) {
        let err: Value = serde_json::from_slice(response.body()).unwrap();
        assert_eq!(response.status().as_u16(), status_code);
        assert_eq!(err.get("status").unwrap(), status_code);
        assert_eq!(err.get("detail").unwrap(), message);
    }

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
        use bytes::Bytes;
        use serde_json::json;

        const OUR_SCALE: u8 = 11;

        async fn settlement_call<F>(
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
                .path(&format!("/accounts/{}/settlements", id))
                .body(json!(Quantity::new(amount.to_string(), scale)).to_string());

            if let Some(idempotency_key) = idempotency_key {
                response = response.header("Idempotency-Key", idempotency_key);
            }
            response.reply(api).await
        }

        #[tokio::test]
        async fn settlement_ok() {
            let id = TEST_ACCOUNT_0.clone().id.to_string();
            let store = test_store(false, true);
            let api = test_api(store.clone(), false);

            // The operator accounts are configured to work with CONNECTOR_SCALE
            // = 9. When
            // we send a settlement with scale OUR_SCALE, the connector should respond
            // with 2 less 0's.
            let response = settlement_call(&api, &id, 200, OUR_SCALE, Some(IDEMPOTENCY)).await;
            assert_eq!(response.body(), &Bytes::from("RECEIVED"));
            assert_eq!(response.status(), StatusCode::CREATED);

            // check that it's idempotent
            let response = settlement_call(&api, &id, 200, OUR_SCALE, Some(IDEMPOTENCY)).await;
            assert_eq!(response.body(), &Bytes::from("RECEIVED"));
            assert_eq!(response.status(), StatusCode::CREATED);

            // fails with different account id
            let response = settlement_call(&api, "2", 200, OUR_SCALE, Some(IDEMPOTENCY)).await;
            check_error_status_and_message(
                response,
                409,
                "Provided idempotency key is tied to other input",
            );

            // fails with different settlement data and account id
            let response = settlement_call(&api, "2", 42, OUR_SCALE, Some(IDEMPOTENCY)).await;
            check_error_status_and_message(
                response,
                409,
                "Provided idempotency key is tied to other input",
            );

            // fails with different settlement data and same account id
            let response = settlement_call(&api, &id, 42, OUR_SCALE, Some(IDEMPOTENCY)).await;
            check_error_status_and_message(
                response,
                409,
                "Provided idempotency key is tied to other input",
            );

            // works without idempotency key
            let response = settlement_call(&api, &id, 400, OUR_SCALE, None).await;
            assert_eq!(response.body(), &Bytes::from("RECEIVED"));
            assert_eq!(response.status(), StatusCode::CREATED);

            let s = store.clone();
            let cache = s.cache.read();
            let cached_data = cache.get(&IDEMPOTENCY.to_owned()).unwrap();

            let cache_hits = s.cache_hits.read();
            assert_eq!(*cache_hits, 4);
            assert_eq!(cached_data.status, StatusCode::CREATED);
            assert_eq!(cached_data.body, &bytes::Bytes::from("RECEIVED"));
        }

        #[tokio::test]
        // The connector must save the difference each time there's precision
        // loss and try to add it the amount it's being notified to settle for the next time.
        async fn settlement_leftovers() {
            let id = TEST_ACCOUNT_0.clone().id.to_string();
            let store = test_store(false, true);
            let api = test_api(store.clone(), false);

            // Send 205 with scale 11, 2 decimals lost -> 0.05 leftovers
            let response = settlement_call(&api, &id, 205, 11, None).await;
            assert_eq!(response.body(), &Bytes::from("RECEIVED"));

            // balance should be 2
            assert_eq!(store.get_balance(TEST_ACCOUNT_0.id), 2);
            assert_eq!(
                store
                    .get_uncredited_settlement_amount(TEST_ACCOUNT_0.id)
                    .await
                    .unwrap(),
                (BigUint::from(5u32), 11)
            );

            // Send 855 with scale 12, 3 decimals lost -> 0.855 leftovers,
            let response = settlement_call(&api, &id, 855, 12, None).await;
            assert_eq!(response.body(), &Bytes::from("RECEIVED"));
            assert_eq!(store.get_balance(TEST_ACCOUNT_0.id), 2);
            // total leftover: 0.905 = 0.05 + 0.855
            assert_eq!(
                store
                    .get_uncredited_settlement_amount(TEST_ACCOUNT_0.id)
                    .await
                    .unwrap(),
                (BigUint::from(905u32), 12)
            );

            // send 110 with scale 11, 2 decimals lost -> 0.1 leftover
            let response = settlement_call(&api, &id, 110, 11, None).await;
            assert_eq!(response.body(), &Bytes::from("RECEIVED"));
            assert_eq!(store.get_balance(TEST_ACCOUNT_0.id), 3);
            assert_eq!(
                store
                    .get_uncredited_settlement_amount(TEST_ACCOUNT_0.id)
                    .await
                    .unwrap(),
                (BigUint::from(1005u32), 12)
            );

            // send 5 with scale 9, will consume the leftovers and increase
            // total balance by 6 while updating the rest of the leftovers
            let response = settlement_call(&api, &id, 5, 9, None).await;
            assert_eq!(response.body(), &Bytes::from("RECEIVED"));
            assert_eq!(store.get_balance(TEST_ACCOUNT_0.id), 9);
            assert_eq!(
                store
                    .get_uncredited_settlement_amount(TEST_ACCOUNT_0.id)
                    .await
                    .unwrap(),
                (BigUint::from(5u32), 12)
            );

            // we send a payment with a smaller scale than the account now
            let response = settlement_call(&api, &id, 2, 7, None).await;
            assert_eq!(response.body(), &Bytes::from("RECEIVED"));
            assert_eq!(store.get_balance(TEST_ACCOUNT_0.id), 209);
            // leftovers are still the same
            assert_eq!(
                store
                    .get_uncredited_settlement_amount(TEST_ACCOUNT_0.id)
                    .await
                    .unwrap(),
                (BigUint::from(5u32), 12)
            );
        }

        #[tokio::test]
        async fn account_has_no_engine_configured() {
            let id = TEST_ACCOUNT_0.clone().id.to_string();
            let store = test_store(false, false);
            let api = test_api(store.clone(), false);

            let response = settlement_call(&api, &id, 100, 18, Some(IDEMPOTENCY)).await;
            check_error_status_and_message(response, 404, "Account 00000000-0000-0000-0000-000000000000 has no settlement engine details configured, cannot send a settlement engine message to that account");

            // check that it's idempotent
            let response = settlement_call(&api, &id, 100, 18, Some(IDEMPOTENCY)).await;
            check_error_status_and_message(response, 404, "Account 00000000-0000-0000-0000-000000000000 has no settlement engine details configured, cannot send a settlement engine message to that account");

            let s = store.clone();
            let cache = s.cache.read();
            let cached_data = cache.get(IDEMPOTENCY).unwrap();

            let cache_hits = s.cache_hits.read();
            assert_eq!(*cache_hits, 1);
            assert_eq!(cached_data.status, 404);
            assert_eq!(cached_data.body, &bytes::Bytes::from("Account 00000000-0000-0000-0000-000000000000 has no settlement engine details configured, cannot send a settlement engine message to that account"));
        }

        #[tokio::test]
        async fn update_balance_for_incoming_settlement_fails() {
            let id = TEST_ACCOUNT_0.clone().id.to_string();
            let store = test_store(true, true);
            let api = test_api(store, false);
            let response = settlement_call(&api, &id, 100, 18, None).await;
            assert_eq!(response.status().as_u16(), 500);
        }

        #[tokio::test]
        async fn invalid_account_id() {
            // the api is configured to take an accountId type
            // supplying an id that cannot be parsed to that type must fail
            let id = "a".to_string();
            let store = test_store(false, true);
            let api = test_api(store.clone(), false);

            let ret = settlement_call(&api, &id, 100, 18, Some(IDEMPOTENCY)).await;
            check_error_status_and_message(ret, 400, "a is an invalid account ID");

            // check that it's idempotent
            let ret = settlement_call(&api, &id, 100, 18, Some(IDEMPOTENCY)).await;
            check_error_status_and_message(ret, 400, "a is an invalid account ID");

            let _ret = settlement_call(&api, &id, 100, 18, Some(IDEMPOTENCY)).await;

            let s = store.clone();
            let cache = s.cache.read();
            let cached_data = cache.get(IDEMPOTENCY).unwrap();

            let cache_hits = s.cache_hits.read();
            assert_eq!(*cache_hits, 2);
            assert_eq!(cached_data.status, 400);
            assert_eq!(
                cached_data.body,
                &bytes::Bytes::from("a is an invalid account ID")
            );
        }

        #[tokio::test]
        async fn account_not_in_store() {
            let id = TEST_ACCOUNT_0.clone().id.to_string();
            let store = TestStore::new(vec![], false);
            let api = test_api(store.clone(), false);

            let ret = settlement_call(&api, &id, 100, 18, Some(IDEMPOTENCY)).await;
            check_error_status_and_message(
                ret,
                404,
                "Account 00000000-0000-0000-0000-000000000000 was not found",
            );

            let ret = settlement_call(&api, &id, 100, 18, Some(IDEMPOTENCY)).await;
            check_error_status_and_message(
                ret,
                404,
                "Account 00000000-0000-0000-0000-000000000000 was not found",
            );

            let s = store.clone();
            let cache = s.cache.read();
            let cached_data = cache.get(IDEMPOTENCY).unwrap();
            let cache_hits = s.cache_hits.read();
            assert_eq!(*cache_hits, 1);
            assert_eq!(cached_data.status, 404);
            assert_eq!(
                cached_data.body,
                &bytes::Bytes::from("Account 00000000-0000-0000-0000-000000000000 was not found")
            );
        }
    }

    mod message_tests {
        use super::*;
        use bytes::Bytes;

        async fn messages_call<F>(
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
                .path(&format!("/accounts/{}/messages", id))
                .body(message);

            if let Some(idempotency_key) = idempotency_key {
                response = response.header("Idempotency-Key", idempotency_key);
            }
            response.reply(api).await
        }

        #[tokio::test]
        async fn message_ok() {
            let id = TEST_ACCOUNT_0.clone().id.to_string();
            let store = test_store(false, true);
            let api = test_api(store.clone(), true);

            let ret = messages_call(&api, &id, &[], Some(IDEMPOTENCY)).await;
            assert_eq!(ret.status(), StatusCode::CREATED);
            assert_eq!(ret.body(), &Bytes::from("hello!"));

            let ret = messages_call(&api, &id, &[], Some(IDEMPOTENCY)).await;
            assert_eq!(ret.status(), StatusCode::CREATED);
            assert_eq!(ret.body(), &Bytes::from("hello!"));

            // Using the same idempotency key with different arguments MUST
            // fail.
            let ret = messages_call(&api, "1", &[], Some(IDEMPOTENCY)).await;
            check_error_status_and_message(
                ret,
                409,
                "Provided idempotency key is tied to other input",
            );

            let data = [0, 1, 2];
            // fails with different account id and data
            let ret = messages_call(&api, "1", &data[..], Some(IDEMPOTENCY)).await;
            check_error_status_and_message(
                ret,
                409,
                "Provided idempotency key is tied to other input",
            );

            // fails for same account id but different data
            let ret = messages_call(&api, &id, &data[..], Some(IDEMPOTENCY)).await;
            check_error_status_and_message(
                ret,
                409,
                "Provided idempotency key is tied to other input",
            );

            let s = store.clone();
            let cache = s.cache.read();
            let cached_data = cache.get(IDEMPOTENCY).unwrap();
            let cache_hits = s.cache_hits.read();
            assert_eq!(*cache_hits, 4);
            assert_eq!(cached_data.status, StatusCode::CREATED);
            assert_eq!(cached_data.body, &bytes::Bytes::from("hello!"));
        }

        #[tokio::test]
        async fn message_gets_rejected() {
            let id = TEST_ACCOUNT_0.clone().id.to_string();
            let store = test_store(false, true);
            let api = test_api(store.clone(), false);

            let ret = messages_call(&api, &id, &[], Some(IDEMPOTENCY)).await;
            check_error_status_and_message(ret, 502, "Error sending message to peer settlement engine. Packet rejected with code: F02, message: No other outgoing handler!");

            let ret = messages_call(&api, &id, &[], Some(IDEMPOTENCY)).await;
            check_error_status_and_message(ret, 502, "Error sending message to peer settlement engine. Packet rejected with code: F02, message: No other outgoing handler!");

            let s = store.clone();
            let cache = s.cache.read();
            let cached_data = cache.get(IDEMPOTENCY).unwrap();
            let cache_hits = s.cache_hits.read();
            assert_eq!(*cache_hits, 1);
            assert_eq!(cached_data.status, 502);
            assert_eq!(cached_data.body, &bytes::Bytes::from("Error sending message to peer settlement engine. Packet rejected with code: F02, message: No other outgoing handler!"));
        }

        #[tokio::test]
        async fn invalid_account_id() {
            // the api is configured to take an accountId type
            // supplying an id that cannot be parsed to that type must fail
            let id = "a".to_string();
            let store = test_store(false, true);
            let api = test_api(store.clone(), true);

            let ret = messages_call(&api, &id, &[], Some(IDEMPOTENCY)).await;
            check_error_status_and_message(ret, 400, "a is an invalid account ID");

            let ret = messages_call(&api, &id, &[], Some(IDEMPOTENCY)).await;
            check_error_status_and_message(ret, 400, "a is an invalid account ID");

            let ret = messages_call(&api, &id, &[], Some(IDEMPOTENCY)).await;
            check_error_status_and_message(ret, 400, "a is an invalid account ID");

            let s = store.clone();
            let cache = s.cache.read();
            let cached_data = cache.get(IDEMPOTENCY).unwrap();

            let cache_hits = s.cache_hits.read();
            assert_eq!(*cache_hits, 2);
            assert_eq!(cached_data.status, 400);
            assert_eq!(
                cached_data.body,
                &bytes::Bytes::from("a is an invalid account ID")
            );
        }

        #[tokio::test]
        async fn account_not_in_store() {
            let id = TEST_ACCOUNT_0.clone().id.to_string();
            let store = TestStore::new(vec![], false);
            let api = test_api(store.clone(), true);

            let ret = messages_call(&api, &id, &[], Some(IDEMPOTENCY)).await;
            check_error_status_and_message(
                ret,
                404,
                "Account 00000000-0000-0000-0000-000000000000 was not found",
            );

            let ret = messages_call(&api, &id, &[], Some(IDEMPOTENCY)).await;
            check_error_status_and_message(
                ret,
                404,
                "Account 00000000-0000-0000-0000-000000000000 was not found",
            );

            let s = store.clone();
            let cache = s.cache.read();
            let cached_data = cache.get(IDEMPOTENCY).unwrap();

            let cache_hits = s.cache_hits.read();
            assert_eq!(*cache_hits, 1);
            assert_eq!(cached_data.status, 404);
            assert_eq!(
                cached_data.body,
                &bytes::Bytes::from("Account 00000000-0000-0000-0000-000000000000 was not found")
            );
        }
    }
}
