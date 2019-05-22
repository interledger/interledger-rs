//! # interledger-stream
//!
//! Client and server implementations of the Interledger [STREAM](https://github.com/interledger/rfcs/blob/master/0029-stream/0029-stream.md) transport protocol.
//!
//! STREAM is responsible for splitting larger payments and messages into smaller chunks of money and data, and sending them over ILP.
#[macro_use]
extern crate log;
#[cfg(test)]
#[macro_use]
extern crate lazy_static;
#[macro_use]
extern crate failure;

mod client;
mod congestion;
mod crypto;
mod error;
mod packet;
mod server;

pub use client::send_money;
pub use error::Error;
pub use server::{ConnectionGenerator, StreamReceiverService};

#[cfg(test)]
pub mod test_helpers {
    use bytes::Bytes;
    use futures::{future::ok, Future};
    use hashbrown::HashMap;
    use interledger_ildcp::IldcpAccount;
    use interledger_router::RouterStore;
    use interledger_service::{Account, AccountStore};
    use std::iter::FromIterator;

    #[derive(Debug, Eq, PartialEq, Clone)]
    pub struct TestAccount {
        pub id: u64,
        pub ilp_address: Bytes,
        pub asset_scale: u8,
        pub asset_code: String,
    }

    impl Account for TestAccount {
        type AccountId = u64;

        fn id(&self) -> u64 {
            self.id
        }
    }

    impl IldcpAccount for TestAccount {
        fn asset_code(&self) -> &str {
            self.asset_code.as_str()
        }

        fn asset_scale(&self) -> u8 {
            self.asset_scale
        }

        fn client_address(&self) -> &[u8] {
            &self.ilp_address[..]
        }
    }

    #[derive(Clone)]
    pub struct TestStore {
        pub route: (Bytes, TestAccount),
    }

    impl AccountStore for TestStore {
        type Account = TestAccount;

        fn get_accounts(
            &self,
            _account_ids: Vec<<<Self as AccountStore>::Account as Account>::AccountId>,
        ) -> Box<Future<Item = Vec<TestAccount>, Error = ()> + Send> {
            Box::new(ok(vec![self.route.1.clone()]))
        }
    }

    impl RouterStore for TestStore {
        fn routing_table(&self) -> HashMap<Bytes, u64> {
            HashMap::from_iter(vec![(self.route.0.clone(), self.route.1.id())].into_iter())
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
    use interledger_packet::{ErrorCode, RejectBuilder};
    use interledger_router::Router;
    use interledger_service::outgoing_service_fn;
    use tokio::runtime::Runtime;

    #[test]
    fn send_money_test() {
        let server_secret = Bytes::from(&[0; 32][..]);
        let destination_address = Bytes::from("example.receiver");
        let account = TestAccount {
            id: 0,
            ilp_address: destination_address.clone(),
            asset_code: "XYZ".to_string(),
            asset_scale: 9,
        };
        let store = TestStore {
            route: (destination_address.clone(), account),
        };
        let connection_generator = ConnectionGenerator::new(server_secret.clone());
        let server = StreamReceiverService::new(
            server_secret,
            outgoing_service_fn(|_| {
                Err(RejectBuilder {
                    code: ErrorCode::F02_UNREACHABLE,
                    message: b"No other outgoing handler",
                    triggered_by: b"example.receiver",
                    data: &[],
                }
                .build())
            }),
        );
        let server = Router::new(store, server);
        let server = IldcpService::new(server);

        let (destination_account, shared_secret) =
            connection_generator.generate_address_and_secret(&destination_address[..]);

        let run = send_money(
            server,
            &test_helpers::TestAccount {
                id: 0,
                asset_code: "XYZ".to_string(),
                asset_scale: 9,
                ilp_address: Bytes::from("example.receiver"),
            },
            &destination_account[..],
            &shared_secret[..],
            100,
        )
        .and_then(|(delivered_amount, _service)| {
            assert_eq!(delivered_amount, 100);
            Ok(())
        })
        .map_err(|err| panic!(err));
        let runtime = Runtime::new().unwrap();
        runtime.block_on_all(run).unwrap();
    }
}
