use crate::SettlementEngine;
use futures::Future;
use hyper::Response;
use interledger_settlement::SettlementData;
use tower_web::{net::ConnectionStream, ServiceBuilder};

/// # Settlement Engine API
///
/// Tower_Web service which exposes settlement related endpoints as described in RFC536,
/// See [forum discussion](https://forum.interledger.org/t/settlement-architecture/545) for more context.
pub struct SettlementEngineApi<E> {
    engine: E,
}

impl_web! {
    impl<E> SettlementEngineApi<E> where E: SettlementEngine + Send + Sync + 'static
    {
        /// Create a new API service by providing it with a field that
        /// implements the `SettlementEngine` trait
        pub fn new(engine: E) -> Self {
            Self { engine }
        }

        #[post("/accounts/:account_id/settlement")]
        /// Forwards the data to the API engine's `send_money` function.
        /// Endpoint: POST /accounts/:id/settlement
        fn execute_settlement(&self, account_id: String, body: SettlementData, idempotency_key: Option<String>) -> impl Future<Item = Response<String>, Error = Response<String>> {
            self.engine.send_money(account_id, body, idempotency_key)
        }

        #[post("/accounts/:account_id/messages")]
        /// Forwards the data to the API engine's `receive_message` function.
        /// Endpoint: POST /accounts/:id/messages
        fn receive_message(&self, account_id: String, body: Vec<u8>, idempotency_key: Option<String>) -> impl Future<Item = Response<String>, Error = Response<String>> {
            self.engine.receive_message(account_id, body, idempotency_key)
        }

        #[post("/accounts/:account_id")]
        /// Forwards the data to the API engine's `create_account` function.
        /// Endpoint: POST /accounts/:id/
        fn create_account(&self, account_id: String, idempotency_key: Option<String>) -> impl Future<Item = Response<String>, Error = Response<String>> {
            self.engine.create_account(account_id, idempotency_key)
        }

    }
}

impl<E> SettlementEngineApi<E>
where
    E: SettlementEngine + Send + Sync + 'static,
{
    /// Serves the API
    /// Example:
    /// ```rust,compile_fail
    /// let settlement_api = SettlementEngineApi::new(engine);
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
