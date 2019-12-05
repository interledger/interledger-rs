//! # interledger-btp
//!
//! Client and server implementations of the [Bilateral Transport Protocol (BTP)](https://github.com/interledger/rfcs/blob/master/0023-bilateral-transfer-protocol/0023-bilateral-transfer-protocol.md).
//! This is a WebSocket-based protocol for exchanging ILP packets between directly connected peers.
//!
//! Because this protocol uses WebSockets, only one party needs to have a publicly-accessible HTTPS
//! endpoint but both sides can send and receive ILP packets.

use futures::Future;
use interledger_service::{Account, Username};
use url::Url;

mod client;
mod errors;
mod oer;
mod packet;
mod server;
mod service;

pub use self::client::{connect_client, connect_to_service_account, parse_btp_url};
pub use self::server::btp_service_as_filter;
pub use self::service::{BtpOutgoingService, BtpService};

pub trait BtpAccount: Account {
    fn get_ilp_over_btp_url(&self) -> Option<&Url>;
    fn get_ilp_over_btp_outgoing_token(&self) -> Option<&[u8]>;
}

/// The interface for Store implementations that can be used with the BTP Server.
pub trait BtpStore {
    type Account: BtpAccount;

    /// Load Account details based on the auth token received via BTP.
    fn get_account_from_btp_auth(
        &self,
        username: &Username,
        token: &str,
    ) -> Box<dyn Future<Item = Self::Account, Error = ()> + Send>;

    /// Load accounts that have a ilp_over_btp_url configured
    fn get_btp_outgoing_accounts(
        &self,
    ) -> Box<dyn Future<Item = Vec<Self::Account>, Error = ()> + Send>;
}

#[cfg(test)]
mod client_server {
    use super::*;
    use futures::future::{err, lazy, ok, result};
    use interledger_packet::{Address, ErrorCode, FulfillBuilder, PrepareBuilder, RejectBuilder};
    use interledger_service::*;
    use net2::TcpBuilder;
    use std::str::FromStr;
    use std::{
        net::SocketAddr,
        sync::Arc,
        time::{Duration, SystemTime},
    };
    use tokio::runtime::Runtime;
    use uuid::Uuid;

    use lazy_static::lazy_static;

    fn get_open_port() -> SocketAddr {
        for _i in 0..1000 {
            let listener = TcpBuilder::new_v4().unwrap();
            listener.reuse_address(true).unwrap();
            if let Ok(listener) = listener.bind("127.0.0.1:0") {
                return listener.listen(1).unwrap().local_addr().unwrap();
            }
        }
        panic!("Cannot find open port!");
    }

    lazy_static! {
        pub static ref ALICE: Username = Username::from_str("alice").unwrap();
        pub static ref EXAMPLE_ADDRESS: Address = Address::from_str("example.alice").unwrap();
    }

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
            if let Some(ref token) = self.ilp_over_btp_outgoing_token {
                Some(token.as_bytes())
            } else {
                None
            }
        }
    }

    #[derive(Clone)]
    pub struct TestStore {
        accounts: Arc<Vec<TestAccount>>,
    }

    impl AccountStore for TestStore {
        type Account = TestAccount;

        fn get_accounts(
            &self,
            account_ids: Vec<Uuid>,
        ) -> Box<dyn Future<Item = Vec<Self::Account>, Error = ()> + Send> {
            let accounts: Vec<TestAccount> = self
                .accounts
                .iter()
                .filter_map(|account| {
                    if account_ids.contains(&account.id) {
                        Some(account.clone())
                    } else {
                        None
                    }
                })
                .collect();
            if accounts.len() == account_ids.len() {
                Box::new(ok(accounts))
            } else {
                Box::new(err(()))
            }
        }

        // stub implementation (not used in these tests)
        fn get_account_id_from_username(
            &self,
            _username: &Username,
        ) -> Box<dyn Future<Item = Uuid, Error = ()> + Send> {
            Box::new(ok(Uuid::new_v4()))
        }
    }

    impl BtpStore for TestStore {
        type Account = TestAccount;

        fn get_account_from_btp_auth(
            &self,
            username: &Username,
            token: &str,
        ) -> Box<dyn Future<Item = Self::Account, Error = ()> + Send> {
            Box::new(result(
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
                    .ok_or(()),
            ))
        }

        fn get_btp_outgoing_accounts(
            &self,
        ) -> Box<dyn Future<Item = Vec<TestAccount>, Error = ()> + Send> {
            Box::new(ok(self
                .accounts
                .iter()
                .filter(|account| account.ilp_over_btp_url.is_some())
                .cloned()
                .collect()))
        }
    }

    // TODO should this be an integration test, since it binds to a port?
    #[test]
    fn client_server_test() {
        let mut runtime = Runtime::new().unwrap();
        runtime
            .block_on(lazy(|| {
                let bind_addr = get_open_port();

                let server_store = TestStore {
                    accounts: Arc::new(vec![TestAccount {
                        id: Uuid::new_v4(),
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
                let filter = btp_service_as_filter(btp_service.clone(), server_store);
                btp_service.handle_incoming(incoming_service_fn(|_| {
                    Ok(FulfillBuilder {
                        fulfillment: &[0; 32],
                        data: b"test data",
                    }
                    .build())
                }));

                let account = TestAccount {
                    id: Uuid::new_v4(),
                    ilp_over_btp_url: Some(
                        Url::parse(&format!("btp+ws://{}/accounts/alice/ilp/btp", bind_addr))
                            .unwrap(),
                    ),
                    ilp_over_btp_outgoing_token: Some("test_auth_token".to_string()),
                    ilp_over_btp_incoming_token: None,
                };
                let accounts = vec![account.clone()];
                let addr = Address::from_str("example.address").unwrap();
                let addr_clone = addr.clone();
                let client = connect_client(
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
                .and_then(move |btp_service| {
                    let mut btp_service =
                        btp_service.handle_incoming(incoming_service_fn(move |_| {
                            Err(RejectBuilder {
                                code: ErrorCode::F02_UNREACHABLE,
                                message: &[],
                                data: &[],
                                triggered_by: Some(&addr),
                            }
                            .build())
                        }));
                    let btp_service_clone = btp_service.clone();
                    btp_service
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
                        .map_err(|reject| println!("Packet was rejected: {:?}", reject))
                        .and_then(move |_| {
                            btp_service_clone.close();
                            Ok(())
                        })
                });
                let server = warp::serve(filter);
                tokio::spawn(server.bind(bind_addr));
                client
            }))
            .unwrap();
    }
}
