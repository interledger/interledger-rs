//! # interledger-stream
//!
//! Client and server implementations of the Interledger [STREAM](https://github.com/interledger/rfcs/blob/master/0029-stream/0029-stream.md) transport protocol.
//!
//! STREAM is responsible for splitting larger payments and messages into smaller chunks of money and data, and sending them over ILP.

/// Stream client
mod client;
/// Congestion controller consumed by the [stream client](./client/fn.send_money.html)
mod congestion;
/// Cryptographic utilities for generating fulfillments and encrypting/decrypting STREAM packets
mod crypto;
/// Stream errors
mod error;
/// Stream Packet implementation, [as specified in the RFC](https://interledger.org/rfcs/0029-stream/#5-packet-and-frame-specification)
mod packet;
/// A stream server implementing an [Outgoing Service](../interledger_service/trait.OutgoingService.html) for receiving STREAM payments from peers
mod server;

pub use client::{send_money, StreamDelivery};
pub use error::Error;
pub use server::{
    ConnectionGenerator, PaymentNotification, StreamNotificationsStore, StreamReceiverService,
};

#[cfg(test)]
pub mod test_helpers {
    use super::*;
    use async_trait::async_trait;
    use futures::channel::mpsc::UnboundedSender;
    use interledger_errors::{AccountStoreError, AddressStoreError, ExchangeRateStoreError};
    use interledger_packet::Address;
    use interledger_rates::ExchangeRateStore;
    use interledger_router::RouterStore;
    use interledger_service::{Account, AccountStore, AddressStore, Username};
    use interledger_service_util::MaxPacketAmountAccount;
    use once_cell::sync::Lazy;
    use std::collections::HashMap;
    use std::iter::FromIterator;
    use std::str::FromStr;
    use std::sync::Arc;
    use uuid::Uuid;

    pub static EXAMPLE_CONNECTOR: Lazy<Address> =
        Lazy::new(|| Address::from_str("example.connector").unwrap());
    pub static EXAMPLE_RECEIVER: Lazy<Address> =
        Lazy::new(|| Address::from_str("example.receiver").unwrap());
    pub static ALICE: Lazy<Username> = Lazy::new(|| Username::from_str("alice").unwrap());

    #[derive(Debug, Eq, PartialEq, Clone)]
    pub struct TestAccount {
        pub id: Uuid,
        pub ilp_address: Address,
        pub asset_scale: u8,
        pub asset_code: String,
        pub max_packet_amount: Option<u64>,
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

    impl MaxPacketAmountAccount for TestAccount {
        fn max_packet_amount(&self) -> u64 {
            self.max_packet_amount.unwrap_or(std::u64::MAX)
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
        pub route: Option<(String, TestAccount)>,
        pub price_1: Option<f64>,
        pub price_2: Option<f64>,
    }

    #[async_trait]
    impl AccountStore for TestStore {
        type Account = TestAccount;

        async fn get_accounts(
            &self,
            _account_ids: Vec<Uuid>,
        ) -> Result<Vec<Self::Account>, AccountStoreError> {
            Ok(vec![self.route.clone().unwrap().1])
        }

        // stub implementation (not used in these tests)
        async fn get_account_id_from_username(
            &self,
            _username: &Username,
        ) -> Result<Uuid, AccountStoreError> {
            Ok(Uuid::new_v4())
        }
    }

    impl RouterStore for TestStore {
        fn routing_table(&self) -> Arc<HashMap<String, Uuid>> {
            Arc::new(HashMap::from_iter(
                vec![(
                    self.route.clone().unwrap().0,
                    self.route.clone().unwrap().1.id(),
                )]
                .into_iter(),
            ))
        }
    }

    #[async_trait]
    impl AddressStore for TestStore {
        /// Saves the ILP Address in the store's memory and database
        async fn set_ilp_address(&self, _ilp_address: Address) -> Result<(), AddressStoreError> {
            unimplemented!()
        }

        async fn clear_ilp_address(&self) -> Result<(), AddressStoreError> {
            unimplemented!()
        }

        /// Get's the store's ilp address from memory
        fn get_ilp_address(&self) -> Address {
            Address::from_str("example.connector").unwrap()
        }
    }

    #[async_trait]
    impl ExchangeRateStore for TestStore {
        fn get_exchange_rates(&self, codes: &[&str]) -> Result<Vec<f64>, ExchangeRateStoreError> {
            match (self.price_1, self.price_2) {
                (Some(price_1), Some(price_2)) => Ok(vec![price_1, price_2]),
                _ => Err(ExchangeRateStoreError::PairNotFound {
                    from: codes[0].to_string(),
                    to: codes[1].to_string(),
                }),
            }
        }

        fn set_exchange_rates(
            &self,
            _rates: HashMap<String, f64>,
        ) -> Result<(), ExchangeRateStoreError> {
            unimplemented!("Cannot set exchange rates")
        }

        fn get_all_exchange_rates(&self) -> Result<HashMap<String, f64>, ExchangeRateStoreError> {
            unimplemented!("Cannot get all exchange rates")
        }
    }
}

#[cfg(test)]
mod send_money_to_receiver {
    use super::test_helpers::*;
    use super::*;
    use bytes::Bytes;
    use interledger_packet::Address;
    use interledger_packet::{ErrorCode, RejectBuilder};
    use interledger_router::Router;
    use interledger_service::outgoing_service_fn;
    use interledger_service_util::ExchangeRateService;
    use std::str::FromStr;
    use uuid::Uuid;

    #[tokio::test]
    async fn send_money_test() {
        let server_secret = Bytes::from(&[0; 32][..]);
        let destination_address = Address::from_str("example.receiver").unwrap();
        let account = TestAccount {
            id: Uuid::new_v4(),
            ilp_address: destination_address.clone(),
            asset_code: "XYZ".to_string(),
            asset_scale: 9,
            max_packet_amount: None,
        };
        let store = TestStore {
            route: Some((destination_address.to_string(), account)),
            price_1: None,
            price_2: None,
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

        let (destination_account, shared_secret) =
            connection_generator.generate_address_and_secret(&destination_address);

        let destination_address = Address::from_str("example.receiver").unwrap();
        let receipt = send_money(
            server,
            &test_helpers::TestAccount {
                id: Uuid::new_v4(),
                asset_code: "XYZ".to_string(),
                asset_scale: 9,
                ilp_address: destination_address,
                max_packet_amount: None,
            },
            TestStore {
                route: None,
                price_1: None,
                price_2: None,
            },
            destination_account,
            &shared_secret[..],
            100,
            0.0,
        )
        .await
        .unwrap();

        assert_eq!(receipt.delivered_amount, 100);
    }

    #[tokio::test]
    async fn payment_fails_if_large_spread() {
        let server_secret = Bytes::from(&[0; 32][..]);
        let source_address = Address::from_str("example.sender").unwrap();
        let destination_address = Address::from_str("example.receiver").unwrap();

        let sender_account = TestAccount {
            id: Uuid::new_v4(),
            ilp_address: source_address.clone(),
            asset_code: "XYZ".to_string(),
            asset_scale: 6,
            max_packet_amount: None,
        };

        let recipient_account = TestAccount {
            id: Uuid::new_v4(),
            ilp_address: destination_address.clone(),
            asset_code: "ABC".to_string(),
            asset_scale: 9,
            max_packet_amount: None,
        };

        let store = TestStore {
            route: Some((destination_address.to_string(), recipient_account)),
            price_1: Some(1.0),
            price_2: Some(1.0),
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

        let server = ExchangeRateService::new(0.02, store.clone(), server);
        let server = Router::new(store.clone(), server);

        let (destination_account, shared_secret) =
            connection_generator.generate_address_and_secret(&destination_address);

        let result = send_money(
            server,
            &sender_account,
            store,
            destination_account,
            &shared_secret[..],
            1000,
            0.014,
        )
        .await;

        // Connector takes 2% spread, but we're only willing to tolerate 1.4%
        match result {
            Err(Error::SendMoneyError(_)) => {}
            _ => panic!("Payment should fail fast due to poor exchange rates"),
        }
    }
}
