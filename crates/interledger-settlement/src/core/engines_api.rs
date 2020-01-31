/// # Settlement Engine API
///
/// Web service which exposes settlement related endpoints as described in the [RFC](https://interledger.org/rfcs/0038-settlement-engines/),
/// All endpoints are idempotent.
use super::{
    get_hash_of,
    idempotency::{make_idempotent_call, IdempotentStore},
    types::{Quantity, SettlementEngine},
};
use bytes::Bytes;
use http::StatusCode;
use hyper::Response;
use interledger_errors::default_rejection_handler;
use serde::{Deserialize, Serialize};
use warp::Filter;

#[derive(Serialize, Deserialize, Debug, Clone, Hash)]
pub struct CreateAccount {
    id: String,
}

/// Makes an idempotent call to [`engine.create_account`](../types/trait.SettlementEngine.html#tymethod.create_account)
/// Returns `Status Code 201`
async fn create_engine_account<E, S>(
    idempotency_key: Option<String>,
    account_id: CreateAccount,
    engine: E,
    store: S,
) -> Result<impl warp::Reply, warp::Rejection>
where
    E: SettlementEngine + Clone + Send + Sync + 'static,
    S: IdempotentStore + Clone + Send + Sync + 'static,
{
    let input_hash = get_hash_of(account_id.id.as_ref());
    let (status_code, message) = make_idempotent_call(
        store,
        engine.create_account(account_id.id),
        input_hash,
        idempotency_key,
        StatusCode::CREATED,
        Bytes::from("CREATED"),
    )
    .await?;

    Ok(Response::builder()
        .header("Content-Type", "application/json")
        .status(status_code)
        .body(message)
        .unwrap())
}

/// Makes an idempotent call to [`engine.delete_account`](../types/trait.SettlementEngine.html#tymethod.delete_account)
/// Returns Status Code `204`.
async fn delete_engine_account<E, S>(
    account_id: String,
    idempotency_key: Option<String>,
    engine: E,
    store: S,
) -> Result<impl warp::Reply, warp::Rejection>
where
    E: SettlementEngine + Clone + Send + Sync + 'static,
    S: IdempotentStore + Clone + Send + Sync + 'static,
{
    let input_hash = get_hash_of(account_id.as_ref());
    let (status_code, message) = make_idempotent_call(
        store,
        engine.delete_account(account_id),
        input_hash,
        idempotency_key,
        StatusCode::NO_CONTENT,
        Bytes::from("DELETED"),
    )
    .await?;

    Ok(Response::builder()
        .header("Content-Type", "application/json")
        .status(status_code)
        .body(message)
        .unwrap())
}

/// Makes an idempotent call to [`engine.send_money`](../types/trait.SettlementEngine.html#tymethod.send_money)
/// Returns Status Code `201`
async fn engine_send_money<E, S>(
    id: String,
    idempotency_key: Option<String>,
    quantity: Quantity,
    engine: E,
    store: S,
) -> Result<impl warp::Reply, warp::Rejection>
where
    E: SettlementEngine + Clone + Send + Sync + 'static,
    S: IdempotentStore + Clone + Send + Sync + 'static,
{
    let input = format!("{}{:?}", id, quantity);
    let input_hash = get_hash_of(input.as_ref());
    let (status_code, message) = make_idempotent_call(
        store,
        engine.send_money(id, quantity),
        input_hash,
        idempotency_key,
        StatusCode::CREATED,
        Bytes::from("EXECUTED"),
    )
    .await?;

    Ok(Response::builder()
        .header("Content-Type", "application/json")
        .status(status_code)
        .body(message)
        .unwrap())
}

/// Makes an idempotent call to [`engine.receive_message`](../types/trait.SettlementEngine.html#tymethod.receive_message)
/// Returns Status Code `201`
async fn engine_receive_message<E, S>(
    id: String,
    idempotency_key: Option<String>,
    message: Bytes,
    engine: E,
    store: S,
) -> Result<impl warp::Reply, warp::Rejection>
where
    E: SettlementEngine + Clone + Send + Sync + 'static,
    S: IdempotentStore + Clone + Send + Sync + 'static,
{
    let input = format!("{}{:?}", id, message);
    let input_hash = get_hash_of(input.as_ref());
    let (status_code, message) = make_idempotent_call(
        store,
        engine.receive_message(id, message.to_vec()),
        input_hash,
        idempotency_key,
        StatusCode::CREATED,
        Bytes::from("RECEIVED"),
    )
    .await?;

    Ok(Response::builder()
        .header("Content-Type", "application/json")
        .status(status_code)
        .body(message)
        .unwrap())
}

/// Returns a Settlement Engine filter which exposes a Warp-compatible
/// idempotent API which forwards calls to the provided settlement engine which
/// uses the underlying store for idempotency.
pub fn create_settlement_engine_filter<E, S>(
    engine: E,
    store: S,
) -> warp::filters::BoxedFilter<(impl warp::Reply,)>
where
    E: SettlementEngine + Clone + Send + Sync + 'static,
    S: IdempotentStore + Clone + Send + Sync + 'static,
{
    let with_store = warp::any().map(move || store.clone()).boxed();
    let with_engine = warp::any().map(move || engine.clone()).boxed();
    let idempotency = warp::header::optional::<String>("idempotency-key");
    let account_id = warp::path("accounts").and(warp::path::param::<String>()); // account_id

    // POST /accounts/ (optional idempotency-key header)
    // Body is a Vec<u8> object
    let accounts = warp::post()
        .and(warp::path("accounts"))
        .and(warp::path::end())
        .and(idempotency)
        .and(warp::body::json())
        .and(with_engine.clone())
        .and(with_store.clone())
        .and_then(create_engine_account);

    // DELETE /accounts/:id (optional idempotency-key header)
    let del_account = warp::delete()
        .and(account_id)
        .and(warp::path::end())
        .and(idempotency)
        .and(with_engine.clone())
        .and(with_store.clone())
        .and_then(delete_engine_account);

    // POST /accounts/:aVcount_id/settlements (optional idempotency-key header)
    // Body is a Quantity object
    let settlement_endpoint = account_id.and(warp::path("settlements"));
    let settlements = warp::post()
        .and(settlement_endpoint)
        .and(warp::path::end())
        .and(idempotency)
        .and(warp::body::json())
        .and(with_engine.clone())
        .and(with_store.clone())
        .and_then(engine_send_money);

    // POST /accounts/:account_id/messages (optional idempotency-key header)
    // Body is a Vec<u8> object
    let messages_endpoint = account_id.and(warp::path("messages"));
    let messages = warp::post()
        .and(messages_endpoint)
        .and(warp::path::end())
        .and(idempotency)
        .and(warp::body::bytes())
        .and(with_engine)
        .and(with_store)
        .and_then(engine_receive_message);

    accounts
        .or(del_account)
        .or(settlements)
        .or(messages)
        .recover(default_rejection_handler)
        .boxed()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::idempotency::IdempotentData;
    use crate::core::types::{ApiResponse, ApiResult};
    use async_trait::async_trait;
    use bytes::Bytes;
    use http::StatusCode;
    use interledger_errors::*;
    use parking_lot::RwLock;
    use serde_json::{json, Value};
    use std::collections::HashMap;
    use std::sync::Arc;

    fn check_error_status_and_message(response: Response<Bytes>, status_code: u16, message: &str) {
        let err: Value = serde_json::from_slice(response.body()).unwrap();
        assert_eq!(response.status().as_u16(), status_code);
        assert_eq!(err.get("status").unwrap(), status_code);
        assert_eq!(err.get("detail").unwrap(), message);
    }

    #[derive(Clone)]
    struct TestEngine;

    #[derive(Debug, Clone)]
    pub struct TestAccount;

    #[derive(Clone)]
    pub struct TestStore {
        #[allow(clippy::all)]
        pub cache: Arc<RwLock<HashMap<String, IdempotentData>>>,
        pub cache_hits: Arc<RwLock<u64>>,
    }

    fn test_store() -> TestStore {
        TestStore {
            cache: Arc::new(RwLock::new(HashMap::new())),
            cache_hits: Arc::new(RwLock::new(0)),
        }
    }

    #[async_trait]
    impl IdempotentStore for TestStore {
        async fn load_idempotent_data(
            &self,
            idempotency_key: String,
        ) -> Result<Option<IdempotentData>, IdempotentStoreError> {
            let cache = self.cache.read();
            if let Some(data) = cache.get(&idempotency_key) {
                let mut guard = self.cache_hits.write();
                *guard += 1; // used to test how many times this branch gets executed
                Ok(Some(data.clone()))
            } else {
                Ok(None)
            }
        }

        async fn save_idempotent_data(
            &self,
            idempotency_key: String,
            input_hash: [u8; 32],
            status_code: StatusCode,
            data: Bytes,
        ) -> Result<(), IdempotentStoreError> {
            let mut cache = self.cache.write();
            cache.insert(
                idempotency_key,
                IdempotentData::new(status_code, data, input_hash),
            );
            Ok(())
        }
    }

    pub static IDEMPOTENCY: &str = "abcd01234";

    #[async_trait]
    impl SettlementEngine for TestEngine {
        async fn send_money(&self, _account_id: String, _money: Quantity) -> ApiResult {
            Ok(ApiResponse::Default)
        }

        async fn receive_message(&self, _account_id: String, _message: Vec<u8>) -> ApiResult {
            Ok(ApiResponse::Default)
        }

        async fn create_account(&self, _account_id: String) -> ApiResult {
            Ok(ApiResponse::Default)
        }

        async fn delete_account(&self, _account_id: String) -> ApiResult {
            Ok(ApiResponse::Default)
        }
    }

    #[tokio::test]
    async fn idempotent_execute_settlement() {
        let store = test_store();
        let engine = TestEngine;
        let api = create_settlement_engine_filter(engine, store.clone());

        let settlement_call = |id, amount, scale| {
            warp::test::request()
                .method("POST")
                .path(&format!("/accounts/{}/settlements", id))
                .body(json!(Quantity::new(amount, scale)).to_string())
                .header("Idempotency-Key", IDEMPOTENCY)
                .reply(&api)
        };

        let ret = settlement_call("1".to_owned(), 100, 6).await;
        assert_eq!(ret.status(), StatusCode::CREATED);
        assert_eq!(ret.body(), "EXECUTED");

        // is idempotent
        let ret = settlement_call("1".to_owned(), 100, 6).await;
        assert_eq!(ret.status(), StatusCode::CREATED);
        assert_eq!(ret.body(), "EXECUTED");

        // fails with different id and same data
        let ret = settlement_call("42".to_owned(), 100, 6).await;
        check_error_status_and_message(ret, 409, "Provided idempotency key is tied to other input");

        // fails with same id and different data
        let ret = settlement_call("1".to_owned(), 42, 6).await;
        check_error_status_and_message(ret, 409, "Provided idempotency key is tied to other input");

        // fails with different id and different data
        let ret = settlement_call("42".to_owned(), 42, 6).await;
        check_error_status_and_message(ret, 409, "Provided idempotency key is tied to other input");

        let cache = store.cache.read();
        let cached_data = cache.get(&IDEMPOTENCY.to_string()).unwrap();

        let cache_hits = store.cache_hits.read();
        assert_eq!(*cache_hits, 4);
        assert_eq!(cached_data.status, 201);
        assert_eq!(cached_data.body, "EXECUTED".to_string());
    }

    #[tokio::test]
    async fn idempotent_receive_message() {
        let store = test_store();
        let engine = TestEngine;
        let api = create_settlement_engine_filter(engine, store.clone());

        let messages_call = |id, msg| {
            warp::test::request()
                .method("POST")
                .path(&format!("/accounts/{}/messages", id))
                .body(msg)
                .header("Idempotency-Key", IDEMPOTENCY)
                .reply(&api)
        };

        let ret = messages_call("1", vec![0]).await;
        assert_eq!(ret.status().as_u16(), StatusCode::CREATED);
        assert_eq!(ret.body(), "RECEIVED");

        // is idempotent
        let ret = messages_call("1", vec![0]).await;
        assert_eq!(ret.status().as_u16(), StatusCode::CREATED);
        assert_eq!(ret.body(), "RECEIVED");

        // fails with different id and same data
        let ret = messages_call("42", vec![0]).await;
        check_error_status_and_message(ret, 409, "Provided idempotency key is tied to other input");

        // fails with same id and different data
        let ret = messages_call("1", vec![42]).await;
        check_error_status_and_message(ret, 409, "Provided idempotency key is tied to other input");

        // fails with different id and different data
        let ret = messages_call("42", vec![42]).await;
        check_error_status_and_message(ret, 409, "Provided idempotency key is tied to other input");

        let cache = store.cache.read();
        let cached_data = cache.get(&IDEMPOTENCY.to_string()).unwrap();

        let cache_hits = store.cache_hits.read();
        assert_eq!(*cache_hits, 4);
        assert_eq!(cached_data.status, 201);
        assert_eq!(cached_data.body, "RECEIVED".to_string());
    }

    #[tokio::test]
    async fn idempotent_create_account() {
        let store = test_store();
        let engine = TestEngine;
        let api = create_settlement_engine_filter(engine, store.clone());

        let create_account_call = |id: &str| {
            warp::test::request()
                .method("POST")
                .path("/accounts")
                .body(json!(CreateAccount { id: id.to_string() }).to_string())
                .header("Idempotency-Key", IDEMPOTENCY)
                .reply(&api)
        };

        let ret = create_account_call("1").await;
        assert_eq!(ret.status().as_u16(), StatusCode::CREATED);
        assert_eq!(ret.body(), "CREATED");

        // is idempotent
        let ret = create_account_call("1").await;
        assert_eq!(ret.status().as_u16(), StatusCode::CREATED);
        assert_eq!(ret.body(), "CREATED");

        // fails with different id
        let ret = create_account_call("42").await;
        check_error_status_and_message(ret, 409, "Provided idempotency key is tied to other input");

        let cache = store.cache.read();
        let cached_data = cache.get(&IDEMPOTENCY.to_string()).unwrap();

        let cache_hits = store.cache_hits.read();
        assert_eq!(*cache_hits, 2);
        assert_eq!(cached_data.status, 201);
        assert_eq!(cached_data.body, "CREATED".to_string());
    }

    #[tokio::test]
    async fn idempotent_delete_account() {
        let store = test_store();
        let engine = TestEngine;
        let api = create_settlement_engine_filter(engine, store.clone());

        let delete_account_call = |id: &str| {
            warp::test::request()
                .method("DELETE")
                .path(&format!("/accounts/{}", id))
                .header("Idempotency-Key", IDEMPOTENCY)
                .reply(&api)
        };

        let ret = delete_account_call("1").await;
        assert_eq!(ret.status(), StatusCode::NO_CONTENT);
        assert_eq!(ret.body(), "DELETED");

        // is idempotent
        let ret = delete_account_call("1").await;
        assert_eq!(ret.status(), StatusCode::NO_CONTENT);
        assert_eq!(ret.body(), "DELETED");

        // fails with different id
        let ret = delete_account_call("42").await;
        check_error_status_and_message(ret, 409, "Provided idempotency key is tied to other input");

        let cache = store.cache.read();
        let cached_data = cache.get(&IDEMPOTENCY.to_string()).unwrap();

        let cache_hits = store.cache_hits.read();
        assert_eq!(*cache_hits, 2);
        assert_eq!(cached_data.status, 204);
        assert_eq!(cached_data.body, "DELETED".to_string());
    }
}
