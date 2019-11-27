//! # interledger-stream
//!
//! Client and server implementations of the Interledger [STREAM](https://github.com/interledger/rfcs/blob/master/0029-stream/0029-stream.md) transport protocol.
//!
//! STREAM is responsible for splitting larger payments and messages into smaller chunks of money and data, and sending them over ILP.

mod client;
mod congestion;
mod crypto;
mod error;
mod packet;
mod server;

pub use client::{send_money, StreamDelivery};
pub use error::Error;
pub use server::{
    ConnectionGenerator, PaymentNotification, StreamNotificationsStore, StreamReceiverService,
};

#[cfg(test)]
pub mod test_helpers {
    use super::*;
    use futures::{future::ok, sync::mpsc::UnboundedSender, Future};
    use interledger_packet::Address;
    use interledger_router::RouterStore;
    use interledger_service::{Account, AccountStore, AddressStore, Username};
    use lazy_static::lazy_static;
    use std::collections::HashMap;
    use std::iter::FromIterator;
    use std::str::FromStr;
    use std::sync::Arc;
    use uuid::Uuid;

    lazy_static! {
        pub static ref EXAMPLE_CONNECTOR: Address = Address::from_str("example.connector").unwrap();
        pub static ref EXAMPLE_RECEIVER: Address = Address::from_str("example.receiver").unwrap();
        pub static ref ALICE: Username = Username::from_str("alice").unwrap();
    }

    #[derive(Debug, Eq, PartialEq, Clone)]
    pub struct TestAccount {
        pub id: Uuid,
        pub ilp_address: Address,
        pub asset_scale: u8,
        pub asset_code: String,
    }

    impl Account for TestAccount {
        fn id(&self) -> Uuid {
            self.id
        }

        fn username(&self) -> &Username {
            &ALICE
        }

        fn asset_code(&self) -> &str {
            self.asset_code.as_str()
        }

        fn asset_scale(&self) -> u8 {
            self.asset_scale
        }

        fn ilp_address(&self) -> &Address {
            &self.ilp_address
        }
    }

    #[derive(Clone)]
    pub struct DummyStore;

    impl super::StreamNotificationsStore for DummyStore {
        type Account = TestAccount;

        fn add_payment_notification_subscription(
            &self,
            _account_id: Uuid,
            _sender: UnboundedSender<PaymentNotification>,
        ) {
        }

        fn publish_payment_notification(&self, _payment: PaymentNotification) {}
    }

    #[derive(Clone)]
    pub struct TestStore {
        pub route: (String, TestAccount),
    }

    impl AccountStore for TestStore {
        type Account = TestAccount;

        fn get_accounts(
            &self,
            _account_ids: Vec<Uuid>,
        ) -> Box<dyn Future<Item = Vec<TestAccount>, Error = ()> + Send> {
            Box::new(ok(vec![self.route.1.clone()]))
        }

        // stub implementation (not used in these tests)
        fn get_account_id_from_username(
            &self,
            _username: &Username,
        ) -> Box<dyn Future<Item = Uuid, Error = ()> + Send> {
            Box::new(ok(Uuid::new_v4()))
        }
    }

    impl RouterStore for TestStore {
        fn routing_table(&self) -> Arc<HashMap<String, Uuid>> {
            Arc::new(HashMap::from_iter(
                vec![(self.route.0.clone(), self.route.1.id())].into_iter(),
            ))
        }
    }

    impl AddressStore for TestStore {
        /// Saves the ILP Address in the store's memory and database
        fn set_ilp_address(
            &self,
            _ilp_address: Address,
        ) -> Box<dyn Future<Item = (), Error = ()> + Send> {
            unimplemented!()
        }

        fn clear_ilp_address(&self) -> Box<dyn Future<Item = (), Error = ()> + Send> {
            unimplemented!()
        }

        /// Get's the store's ilp address from memory
        fn get_ilp_address(&self) -> Address {
            Address::from_str("example.connector").unwrap()
        }
    }
}

#[cfg(test)]
mod send_money_to_receiver {
    use super::test_helpers::*;
    use super::*;
    use bytes::Bytes;
    use futures::Future;
    use interledger_ildcp::IldcpService;
    use interledger_packet::Address;
    use interledger_packet::{ErrorCode, RejectBuilder};
    use interledger_router::Router;
    use interledger_service::outgoing_service_fn;
    use std::str::FromStr;
    use tokio::runtime::Runtime;
    use uuid::Uuid;

    #[test]
    fn send_money_test() {
        let server_secret = Bytes::from(&[0; 32][..]);
        let destination_address = Address::from_str("example.receiver").unwrap();
        let account = TestAccount {
            id: Uuid::new_v4(),
            ilp_address: destination_address.clone(),
            asset_code: "XYZ".to_string(),
            asset_scale: 9,
        };
        let store = TestStore {
            route: (destination_address.to_string(), account),
        };
        let connection_generator = ConnectionGenerator::new(server_secret.clone());
        let server = StreamReceiverService::new(
            server_secret,
            DummyStore,
            outgoing_service_fn(|_| {
                Err(RejectBuilder {
                    code: ErrorCode::F02_UNREACHABLE,
                    message: b"No other outgoing handler",
                    triggered_by: Some(&EXAMPLE_RECEIVER),
                    data: &[],
                }
                .build())
            }),
        );
        let server = Router::new(store, server);
        let server = IldcpService::new(server);

        let (destination_account, shared_secret) =
            connection_generator.generate_address_and_secret(&destination_address);

        let destination_address = Address::from_str("example.receiver").unwrap();
        let run = send_money(
            server,
            &test_helpers::TestAccount {
                id: Uuid::new_v4(),
                asset_code: "XYZ".to_string(),
                asset_scale: 9,
                ilp_address: destination_address,
            },
            destination_account,
            &shared_secret[..],
            100,
        )
        .and_then(|(receipt, _service)| {
            assert_eq!(receipt.delivered_amount, 100);
            Ok(())
        })
        .map_err(|err| panic!(err));
        let runtime = Runtime::new().unwrap();
        runtime.block_on_all(run).unwrap();
    }
}
