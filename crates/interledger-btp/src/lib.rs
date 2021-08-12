//! # interledger-btp
//!
//! Client and server implementations of the [Bilateral Transport Protocol (BTP)](https://github.com/interledger/rfcs/blob/master/0023-bilateral-transfer-protocol/0023-bilateral-transfer-protocol.md).
//! This is a WebSocket-based protocol for exchanging ILP packets between directly connected peers.
//!
//! Because this protocol uses WebSockets, only one party needs to have a publicly-accessible HTTPS
//! endpoint but both sides can send and receive ILP packets.

use async_trait::async_trait;
use interledger_service::{Account, Username};
use url::Url;

mod client;
mod errors;
mod packet;
mod server;
mod service;
mod wrapped_ws;

pub use self::client::{connect_client, connect_to_service_account};
pub use self::server::btp_service_as_filter; // This is consumed only by the node.
pub use self::service::{BtpOutgoingService, BtpService};

use interledger_errors::BtpStoreError;

/// Extension trait for [Account](../interledger_service/trait.Account.html) with [ILP over BTP](https://interledger.org/rfcs/0023-bilateral-transfer-protocol/) related information
pub trait BtpAccount: Account {
    /// Returns the BTP Websockets URL corresponding to this account
    fn get_ilp_over_btp_url(&self) -> Option<&Url>;
    /// Returns the BTP authentication token which is used when initiating a BTP connection
    /// with a peer
    fn get_ilp_over_btp_outgoing_token(&self) -> Option<&[u8]>;
}

/// The interface for Store implementations that can be used with the BTP Server.
#[async_trait]
pub trait BtpStore {
    type Account: BtpAccount;

    /// Load Account details based on the auth token received via BTP.
    async fn get_account_from_btp_auth(
        &self,
        username: &Username,
        token: &str,
    ) -> Result<Self::Account, BtpStoreError>;

    /// Load accounts that have a ilp_over_btp_url configured
    async fn get_btp_outgoing_accounts(&self) -> Result<Vec<Self::Account>, BtpStoreError>;
}

#[cfg(fuzzing)]
pub mod fuzzing {
    pub use crate::errors::BtpPacketError;
    pub use crate::packet::{BtpPacket, Serializable};

    #[cfg(not(feature = "strict"))]
    pub fn roundtrip_btppacket(data: &[u8]) {
        panic!("you need to enable 'strict' feature for roundtrip fuzzing");
    }

    #[cfg(feature = "strict")]
    pub fn roundtrip_btppacket(data: &[u8]) {
        if let Ok(x) = BtpPacket::from_bytes(data) {
            let out = x.to_bytes();

            assert_eq!(data, out);
        }
    }
}

#[cfg(test)]
mod client_server {
    use super::*;
    use interledger_packet::{Address, ErrorCode, FulfillBuilder, PrepareBuilder, RejectBuilder};
    use interledger_service::*;
    use socket2::{Domain, Socket, Type};
    use std::str::FromStr;
    use std::{
        net::SocketAddr,
        sync::Arc,
        time::{Duration, SystemTime},
    };
    use uuid::Uuid;

    use once_cell::sync::Lazy;

    fn get_open_port() -> SocketAddr {
        for _i in 0..1000 {
            let socket = Socket::new(Domain::IPV4, Type::STREAM, None).unwrap();
            socket.reuse_address().unwrap();
            if socket
                .bind(&"127.0.0.1:0".parse::<SocketAddr>().unwrap().into())
                .is_ok()
            {
                let listener = std::net::TcpListener::from(socket);
                return listener.local_addr().unwrap();
            }
        }
        panic!("Cannot find open port!");
    }

    pub static ALICE: Lazy<Username> = Lazy::new(|| Username::from_str("alice").unwrap());
    pub static EXAMPLE_ADDRESS: Lazy<Address> =
        Lazy::new(|| Address::from_str("example.alice").unwrap());

    #[derive(Clone, Debug)]
    pub struct TestAccount {
        pub id: Uuid,
        pub ilp_over_btp_incoming_token: Option<String>,
        pub ilp_over_btp_outgoing_token: Option<String>,
        pub ilp_over_btp_url: Option<Url>,
    }

    impl Account for TestAccount {
        fn id(&self) -> Uuid {
            self.id
        }

        fn username(&self) -> &Username {
            &ALICE
        }

        fn asset_scale(&self) -> u8 {
            9
        }

        fn asset_code(&self) -> &str {
            "XYZ"
        }

        fn ilp_address(&self) -> &Address {
            &EXAMPLE_ADDRESS
        }
    }

    impl BtpAccount for TestAccount {
        fn get_ilp_over_btp_url(&self) -> Option<&Url> {
            self.ilp_over_btp_url.as_ref()
        }

        fn get_ilp_over_btp_outgoing_token(&self) -> Option<&[u8]> {
            self.ilp_over_btp_outgoing_token
                .as_ref()
                .map(|token| token.as_bytes())
        }
    }

    #[derive(Clone)]
    pub struct TestStore {
        accounts: Arc<[TestAccount]>,
    }

    #[async_trait]
    impl BtpStore for TestStore {
        type Account = TestAccount;

        async fn get_account_from_btp_auth(
            &self,
            username: &Username,
            token: &str,
        ) -> Result<Self::Account, BtpStoreError> {
            self.accounts
                .iter()
                .find(|account| {
                    if let Some(account_token) = &account.ilp_over_btp_incoming_token {
                        account_token == token && account.username() == username
                    } else {
                        false
                    }
                })
                .cloned()
                .ok_or_else(|| BtpStoreError::Unauthorized(username.to_string()))
        }

        async fn get_btp_outgoing_accounts(&self) -> Result<Vec<TestAccount>, BtpStoreError> {
            Ok(self
                .accounts
                .iter()
                .filter(|account| account.ilp_over_btp_url.is_some())
                .cloned()
                .collect())
        }
    }

    // TODO should this be an integration test, since it binds to a port?
    #[tokio::test]
    async fn client_server_test() {
        let bind_addr = get_open_port();

        let server_acc_id = Uuid::new_v4();
        let server_store = TestStore {
            accounts: Arc::new([TestAccount {
                id: server_acc_id,
                ilp_over_btp_incoming_token: Some("test_auth_token".to_string()),
                ilp_over_btp_outgoing_token: None,
                ilp_over_btp_url: None,
            }]),
        };
        let server_address = Address::from_str("example.server").unwrap();
        let btp_service = BtpOutgoingService::new(
            server_address.clone(),
            outgoing_service_fn(move |_| {
                Err(RejectBuilder {
                    code: ErrorCode::F02_UNREACHABLE,
                    message: b"No other outgoing handler",
                    triggered_by: Some(&server_address),
                    data: &[],
                }
                .build())
            }),
        );
        btp_service
            .clone()
            .handle_incoming(incoming_service_fn(|_| {
                Ok(FulfillBuilder {
                    fulfillment: &[0; 32],
                    data: b"test data",
                }
                .build())
            }))
            .await;
        let filter = btp_service_as_filter(btp_service.clone(), server_store);
        let server = warp::serve(filter);
        // Spawn the server and listen for incoming connections
        tokio::spawn(server.bind(bind_addr));

        // Try to connect
        let account = TestAccount {
            id: Uuid::new_v4(),
            ilp_over_btp_url: Some(
                Url::parse(&format!("btp+ws://{}/accounts/alice/ilp/btp", bind_addr)).unwrap(),
            ),
            ilp_over_btp_outgoing_token: Some("test_auth_token".to_string()),
            ilp_over_btp_incoming_token: None,
        };
        let accounts = vec![account.clone()];
        let addr = Address::from_str("example.address").unwrap();
        let addr_clone = addr.clone();

        let btp_client = connect_client(
            addr.clone(),
            accounts,
            true,
            outgoing_service_fn(move |_| {
                Err(RejectBuilder {
                    code: ErrorCode::F02_UNREACHABLE,
                    message: &[],
                    data: &[],
                    triggered_by: Some(&addr_clone),
                }
                .build())
            }),
        )
        .await
        .unwrap();

        let mut btp_client = btp_client
            .handle_incoming(incoming_service_fn(move |_| {
                Err(RejectBuilder {
                    code: ErrorCode::F02_UNREACHABLE,
                    message: &[],
                    data: &[],
                    triggered_by: Some(&addr),
                }
                .build())
            }))
            .await;

        let res = btp_client
            .send_request(OutgoingRequest {
                from: account.clone(),
                to: account.clone(),
                original_amount: 100,
                prepare: PrepareBuilder {
                    destination: Address::from_str("example.destination").unwrap(),
                    amount: 100,
                    execution_condition: &[0; 32],
                    expires_at: SystemTime::now() + Duration::from_secs(30),
                    data: b"test data",
                }
                .build(),
            })
            .await;
        assert!(res.is_ok());

        btp_service.close_connection(&server_acc_id);
        // after removing the connection this will fail
        let mut btp_client_clone = btp_client.clone();
        let res = btp_client_clone
            .send_request(OutgoingRequest {
                from: account.clone(),
                to: account.clone(),
                original_amount: 100,
                prepare: PrepareBuilder {
                    destination: Address::from_str("example.destination").unwrap(),
                    amount: 100,
                    execution_condition: &[0; 32],
                    expires_at: SystemTime::now() + Duration::from_secs(30),
                    data: b"test data",
                }
                .build(),
            })
            .await
            .unwrap_err();
        assert_eq!(res.code(), ErrorCode::R00_TRANSFER_TIMED_OUT);

        // now that we have timed out, if we try sending again we'll see that we
        // have no more connections with this user
        let res = btp_client_clone
            .send_request(OutgoingRequest {
                from: account.clone(),
                to: account.clone(),
                original_amount: 100,
                prepare: PrepareBuilder {
                    destination: Address::from_str("example.destination").unwrap(),
                    amount: 100,
                    execution_condition: &[0; 32],
                    expires_at: SystemTime::now() + Duration::from_secs(30),
                    data: b"test data",
                }
                .build(),
            })
            .await
            .unwrap_err();
        assert_eq!(res.code(), ErrorCode::F02_UNREACHABLE);

        btp_service.close();
    }
}
