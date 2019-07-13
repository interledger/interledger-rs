use crate::SettlementEngine;
use futures::Future;
use hyper::Response;
use interledger_settlement::SettlementData;
use tower_web::{net::ConnectionStream, ServiceBuilder};

pub struct SettlementEngineApi<E> {
    engine: E,
}

impl_web! {
    impl<E> SettlementEngineApi<E> where E: SettlementEngine + Send + Sync + 'static
    {
        pub fn new(engine: E) -> Self {
            Self { engine }
        }

        #[post("/accounts/:account_id/settlement")]
        fn execute_settlement(&self, account_id: String, body: SettlementData, idempotency_key: Option<String>) -> impl Future<Item = Response<String>, Error = Response<String>> {
            self.engine.send_money(account_id, body, idempotency_key)
        }

        #[post("/accounts/:account_id/messages")]
        fn receive_message(&self, account_id: String, body: Vec<u8>, idempotency_key: Option<String>) -> impl Future<Item = Response<String>, Error = Response<String>> {
            self.engine.receive_message(account_id, body, idempotency_key)
        }

        #[post("/accounts/:account_id")]
        fn create_account(&self, account_id: String, idempotency_key: Option<String>) -> impl Future<Item = Response<String>, Error = Response<String>> {
            self.engine.create_account(account_id, idempotency_key)
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
