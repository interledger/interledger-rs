use crate::{ApiResponse, CreateAccount, SettlementEngine};
use bytes::Bytes;
use futures::{
    future::{err, ok, Either},
    Future,
};
use hyper::{Response, StatusCode};
use interledger_settlement::Quantity;
use interledger_settlement::{IdempotentData, IdempotentStore};
use log::error;
use ring::digest::{digest, SHA256};
use tokio::executor::spawn;
use tower_web::{net::ConnectionStream, ServiceBuilder};

/// # Settlement Engine API
///
/// Tower_Web service which exposes settlement related endpoints as described in RFC536,
/// See [forum discussion](https://forum.interledger.org/t/settlement-architecture/545) for more context.
/// All endpoints are idempotent.
pub struct SettlementEngineApi<E, S> {
    engine: E,
    store: S,
}

impl_web! {
    impl<E, S> SettlementEngineApi<E, S>
    where
        E: SettlementEngine + Clone + Send + Sync + 'static,
        S: IdempotentStore + Clone + Send + Sync + 'static,
    {
        /// Create a new API service by providing it with a field that
        /// implements the `SettlementEngine` trait
        pub fn new(engine: E, store: S) -> Self {
            Self { engine, store }
        }

        #[post("/accounts/:account_id/settlements")]
        /// Forwards the data to the API engine's `send_money` function.
        /// Endpoint: POST /accounts/:id/settlements
        fn execute_settlement(&self, account_id: String, body: Quantity, idempotency_key: Option<String>) -> impl Future<Item = Response<String>, Error = Response<String>> {
            // check idempotency
            let input = format!("{}{:?}", account_id, body);
            let input_hash = get_hash_of(input.as_ref());
            let account_id = account_id.clone();
            let engine = self.engine.clone();
            let f = move || engine.send_money(account_id, body);
            self.make_idempotent_call(f, input_hash, idempotency_key)
        }

        #[post("/accounts/:account_id/messages")]
        /// Forwards the data to the API engine's `receive_message` function.
        /// Endpoint: POST /accounts/:id/messages
        fn receive_message(&self, account_id: String, body: Vec<u8>, idempotency_key: Option<String>) -> impl Future<Item = Response<String>, Error = Response<String>> {
            let input = format!("{}{:?}", account_id, body);
            let input_hash = get_hash_of(input.as_ref());
            let account_id = account_id.clone();
            let engine = self.engine.clone();
            let f = move || engine.receive_message(account_id, body);
            self.make_idempotent_call(f, input_hash, idempotency_key)
        }

        #[post("/accounts")]
        /// Forwards the data to the API engine's `create_account` function.
        /// Endpoint: POST /accounts/
        fn create_account(&self, body: CreateAccount, idempotency_key: Option<String>) -> impl Future<Item = Response<String>, Error = Response<String>> {
            let input = format!("{:?}", body);
            let input_hash = get_hash_of(input.as_ref());
            let engine = self.engine.clone();
            let f = move || engine.create_account(body);
            self.make_idempotent_call(f, input_hash, idempotency_key)
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
                    let error_msg = "Couldn' load idempotent data".to_owned();
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

        fn make_idempotent_call<F>(&self, f: F, input_hash: [u8; 32], idempotency_key: Option<String>) -> impl Future<Item = Response<String>, Error = Response<String>>
        where F: FnOnce() -> Box<dyn Future<Item = ApiResponse, Error = ApiResponse> + Send> {
            let store = self.store.clone();
            if let Some(idempotency_key) = idempotency_key {
                // If there an idempotency key was provided, check idempotency
                // and the key was not present or conflicting with an existing
                // key, perform the call and save the idempotent return data
                Either::A(
                    self.check_idempotency(idempotency_key.clone(), input_hash)
                    .map_err(|err| Response::builder().status(502).body(err).unwrap())
                    .then(move |ret: Result<(StatusCode, Bytes), Response<String>>| {
                        if let Ok(ret) = ret {
                            let resp = Response::builder().status(ret.0).body(String::from_utf8_lossy(&ret.1).to_string()).unwrap();
                            if ret.0.is_success() {
                                return Either::A(Either::A(ok(resp)))
                            } else {
                                return Either::A(Either::B(err(resp)))
                            }
                        } else {
                            Either::B(
                                f()
                                .map_err({let store = store.clone(); let idempotency_key = idempotency_key.clone(); move |ret: (StatusCode, String)| {
                                    spawn(store.save_idempotent_data(idempotency_key, input_hash, ret.0, Bytes::from(ret.1.clone())));
                                    Response::builder().status(ret.0).body(ret.1).unwrap()
                                }})
                                .and_then(move |ret: (StatusCode, String)| {
                                    store.save_idempotent_data(idempotency_key, input_hash, ret.0, Bytes::from(ret.1.clone()))
                                    .map_err({let ret = ret.clone(); move |_| {
                                        Response::builder().status(ret.0).body(ret.1).unwrap()
                                    }}).and_then(move |_| {
                                        Ok(Response::builder().status(ret.0).body(ret.1).unwrap())
                                    })
                                })
                            )
                        }
                    }))
            } else {
                // otherwise just make the call without any idempotency saves
                Either::B(
                    f()
                    .map_err(move |ret: (StatusCode, String)| {
                        Response::builder().status(ret.0).body(ret.1).unwrap()
                    })
                    .and_then(move |ret: (StatusCode, String)| {
                        Ok(Response::builder().status(ret.0).body(ret.1).unwrap())
                    })
                )
            }

        }
    }
}

fn get_hash_of(preimage: &[u8]) -> [u8; 32] {
    let mut hash = [0; 32];
    hash.copy_from_slice(digest(&SHA256, preimage).as_ref());
    hash
}

impl<E, S> SettlementEngineApi<E, S>
where
    E: SettlementEngine + Clone + Send + Sync + 'static,
    S: IdempotentStore + Clone + Send + Sync + 'static,
{
    /// Serves the API
    /// Example:
    /// ```rust,compile_fail
    /// let settlement_api = SettlementEngineApi::new(engine, store);
    /// let listener = TcpListener::bind(&http_address)
    ///     .expect("Unable to bind to HTTP address");
    /// tokio::spawn(api.serve(listener.incoming()));
    /// ```
    pub fn serve<I>(self, incoming: I) -> impl Future<Item = (), Error = ()>
    where
        I: ConnectionStream,
        I::Item: Send + 'static,
    {
        ServiceBuilder::new().resource(self).serve(incoming)
    }
}

#[cfg(test)]
mod tests {
    use super::super::engines::ethereum_ledger::fixtures::ALICE;
    use super::super::engines::ethereum_ledger::test_helpers::test_store;
    use super::super::stores::test_helpers::store_helpers::block_on;
    use lazy_static::lazy_static;

    use super::*;
    use crate::ApiResponse;

    #[derive(Clone)]
    struct TestEngine;

    lazy_static! {
        pub static ref IDEMPOTENCY: String = String::from("abcd01234");
    }

    impl SettlementEngine for TestEngine {
        fn send_money(
            &self,
            _account_id: String,
            _money: Quantity,
        ) -> Box<dyn Future<Item = ApiResponse, Error = ApiResponse> + Send> {
            Box::new(ok((StatusCode::from_u16(200).unwrap(), "OK".to_string())))
        }

        fn receive_message(
            &self,
            _account_id: String,
            _message: Vec<u8>,
        ) -> Box<dyn Future<Item = ApiResponse, Error = ApiResponse> + Send> {
            Box::new(ok((
                StatusCode::from_u16(200).unwrap(),
                "RECEIVED".to_string(),
            )))
        }

        fn create_account(
            &self,
            _account_id: CreateAccount,
        ) -> Box<dyn Future<Item = ApiResponse, Error = ApiResponse> + Send> {
            Box::new(ok((
                StatusCode::from_u16(201).unwrap(),
                "CREATED".to_string(),
            )))
        }
    }

    #[test]
    fn idempotent_execute_settlement() {
        let store = test_store(ALICE.clone(), false, false, false);
        let engine = TestEngine;;
        let api = SettlementEngineApi {
            store: store.clone(),
            engine,
        };

        let ret: Response<_> = block_on(api.execute_settlement(
            "1".to_owned(),
            Quantity::new(100, 6),
            Some(IDEMPOTENCY.clone()),
        ))
        .unwrap();
        assert_eq!(ret.status().as_u16(), 200);
        assert_eq!(ret.body(), "OK");

        // is idempotent
        let ret: Response<_> = block_on(api.execute_settlement(
            "1".to_owned(),
            Quantity::new(100, 6),
            Some(IDEMPOTENCY.clone()),
        ))
        .unwrap();
        assert_eq!(ret.status().as_u16(), 200);
        assert_eq!(ret.body(), "OK");

        // // fails with different id and same data
        let ret: Response<_> = block_on(api.execute_settlement(
            "42".to_owned(),
            Quantity::new(100, 6),
            Some(IDEMPOTENCY.clone()),
        ))
        .unwrap_err();
        assert_eq!(ret.status().as_u16(), 409);
        assert_eq!(
            ret.body(),
            "Provided idempotency key is tied to other input"
        );

        // fails with same id and different data
        let ret: Response<_> = block_on(api.execute_settlement(
            "1".to_string(),
            Quantity::new(42, 6),
            Some(IDEMPOTENCY.clone()),
        ))
        .unwrap_err();
        assert_eq!(ret.status().as_u16(), 409);
        assert_eq!(
            ret.body(),
            "Provided idempotency key is tied to other input"
        );

        // fails with different id and different data
        let ret: Response<_> = block_on(api.execute_settlement(
            "42".to_string(),
            Quantity::new(42, 6),
            Some(IDEMPOTENCY.clone()),
        ))
        .unwrap_err();
        assert_eq!(ret.status().as_u16(), 409);
        assert_eq!(
            ret.body(),
            "Provided idempotency key is tied to other input"
        );

        let cache = store.cache.read();
        let cached_data = cache.get(&IDEMPOTENCY.to_string()).unwrap();

        let cache_hits = store.cache_hits.read();
        assert_eq!(*cache_hits, 4);
        assert_eq!(cached_data.0, 200);
        assert_eq!(cached_data.1, "OK".to_string());
    }

    #[test]
    fn idempotent_receive_message() {
        let store = test_store(ALICE.clone(), false, false, false);
        let engine = TestEngine;;
        let api = SettlementEngineApi {
            store: store.clone(),
            engine,
        };

        let ret: Response<_> =
            block_on(api.receive_message("1".to_owned(), vec![0], Some(IDEMPOTENCY.clone())))
                .unwrap();
        assert_eq!(ret.status().as_u16(), 200);
        assert_eq!(ret.body(), "RECEIVED");

        // is idempotent
        let ret: Response<_> =
            block_on(api.receive_message("1".to_owned(), vec![0], Some(IDEMPOTENCY.clone())))
                .unwrap();
        assert_eq!(ret.status().as_u16(), 200);
        assert_eq!(ret.body(), "RECEIVED");

        // // fails with different id and same data
        let ret: Response<_> =
            block_on(api.receive_message("42".to_owned(), vec![0], Some(IDEMPOTENCY.clone())))
                .unwrap_err();
        assert_eq!(ret.status().as_u16(), 409);
        assert_eq!(
            ret.body(),
            "Provided idempotency key is tied to other input"
        );

        // fails with same id and different data
        let ret: Response<_> =
            block_on(api.receive_message("1".to_string(), vec![1], Some(IDEMPOTENCY.clone())))
                .unwrap_err();
        assert_eq!(ret.status().as_u16(), 409);
        assert_eq!(
            ret.body(),
            "Provided idempotency key is tied to other input"
        );

        // fails with different id and different data
        let ret: Response<_> =
            block_on(api.receive_message("42".to_string(), vec![1], Some(IDEMPOTENCY.clone())))
                .unwrap_err();
        assert_eq!(ret.status().as_u16(), 409);
        assert_eq!(
            ret.body(),
            "Provided idempotency key is tied to other input"
        );

        let cache = store.cache.read();
        let cached_data = cache.get(&IDEMPOTENCY.to_string()).unwrap();

        let cache_hits = store.cache_hits.read();
        assert_eq!(*cache_hits, 4);
        assert_eq!(cached_data.0, 200);
        assert_eq!(cached_data.1, "RECEIVED".to_string());
    }

    #[test]
    fn idempotent_create_account() {
        let store = test_store(ALICE.clone(), false, false, false);
        let engine = TestEngine;;
        let api = SettlementEngineApi {
            store: store.clone(),
            engine,
        };

        let ret: Response<_> =
            block_on(api.create_account(CreateAccount::new("1"), Some(IDEMPOTENCY.clone())))
                .unwrap();
        assert_eq!(ret.status().as_u16(), 201);
        assert_eq!(ret.body(), "CREATED");

        // is idempotent
        let ret: Response<_> =
            block_on(api.create_account(CreateAccount::new("1"), Some(IDEMPOTENCY.clone())))
                .unwrap();
        assert_eq!(ret.status().as_u16(), 201);
        assert_eq!(ret.body(), "CREATED");

        // fails with different id
        let ret: Response<_> =
            block_on(api.create_account(CreateAccount::new("42"), Some(IDEMPOTENCY.clone())))
                .unwrap_err();
        assert_eq!(ret.status().as_u16(), 409);
        assert_eq!(
            ret.body(),
            "Provided idempotency key is tied to other input"
        );

        let cache = store.cache.read();
        let cached_data = cache.get(&IDEMPOTENCY.to_string()).unwrap();

        let cache_hits = store.cache_hits.read();
        assert_eq!(*cache_hits, 2);
        assert_eq!(cached_data.0, 201);
        assert_eq!(cached_data.1, "CREATED".to_string());
    }

}
