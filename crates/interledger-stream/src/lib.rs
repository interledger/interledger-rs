//! # interledger-stream
//!
//! Client and server implementations of the Interledger [STREAM](https://github.com/interledger/rfcs/blob/master/0029-stream/0029-stream.md) transport protocol.
//!
//! STREAM is responsible for splitting larger payments and messages into smaller chunks of money and data, and sending them over ILP.
#[macro_use]
extern crate log;
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
    use interledger_ildcp::IldcpAccount;
    use interledger_service::Account;

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
        fn asset_code(&self) -> String {
            self.asset_code.clone()
        }

        fn asset_scale(&self) -> u8 {
            self.asset_scale
        }

        fn client_address(&self) -> Bytes {
            self.ilp_address.clone()
        }
    }
}

#[cfg(test)]
mod send_money_to_receiver {
    use super::*;
    use bytes::Bytes;
    use env_logger;
    use futures::Future;
    use interledger_ildcp::{IldcpResponseBuilder, IldcpService};
    use interledger_packet::{ErrorCode, RejectBuilder};
    use interledger_service::incoming_service_fn;
    use tokio::runtime::Runtime;

    #[test]
    fn send_money_test() {
        env_logger::init();
        let server_secret = [0; 32];
        let destination_address = b"example.receiver";
        let ildcp_info = IldcpResponseBuilder {
            client_address: destination_address,
            asset_code: "XYZ",
            asset_scale: 9,
        }
        .build();
        let connection_generator =
            ConnectionGenerator::new(destination_address, &server_secret[..]);
        let server = StreamReceiverService::new(
            &server_secret,
            ildcp_info,
            incoming_service_fn(|_| RejectBuilder::new(ErrorCode::F02_UNREACHABLE)),
        );
        let server = IldcpService::new(server);

        let (destination_account, shared_secret) =
            connection_generator.generate_address_and_secret(b"test");

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
        .and_then(|(amount_delivered, _service)| {
            assert_eq!(amount_delivered, 100);
            Ok(())
        })
        .map_err(|err| panic!(err));
        let runtime = Runtime::new().unwrap();
        runtime.block_on_all(run).unwrap();
    }
}
