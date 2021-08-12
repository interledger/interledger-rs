mod accounts_test;
mod balances_test;
mod btp_test;
mod http_test;
mod notifications;
mod rate_limiting_test;
mod rates_test;
mod routing_test;
mod settlement_test;

mod fixtures {

    use interledger_api::AccountDetails;
    use interledger_packet::Address;
    use interledger_service::Username;
    use once_cell::sync::Lazy;
    use secrecy::SecretString;
    use std::str::FromStr;

    // We are dylan starting a connection with all these accounts
    pub static ACCOUNT_DETAILS_0: Lazy<AccountDetails> = Lazy::new(|| AccountDetails {
        ilp_address: Some(Address::from_str("example.alice").unwrap()),
        username: Username::from_str("alice").unwrap(),
        asset_scale: 6,
        asset_code: "XYZ".to_string(),
        max_packet_amount: 1000,
        min_balance: Some(-1000),
        ilp_over_http_url: Some("http://example.com/accounts/dylan/ilp".to_string()),
        ilp_over_http_incoming_token: Some(SecretString::new("incoming_auth_token".to_string())),
        ilp_over_http_outgoing_token: Some(SecretString::new("outgoing_auth_token".to_string())),
        ilp_over_btp_url: Some("btp+ws://example.com/accounts/dylan/ilp/btp".to_string()),
        ilp_over_btp_incoming_token: Some(SecretString::new("btp_token".to_string())),
        ilp_over_btp_outgoing_token: Some(SecretString::new("btp_token".to_string())),
        settle_threshold: Some(0),
        settle_to: Some(-1000),
        routing_relation: Some("Parent".to_owned()),
        round_trip_time: None,
        amount_per_minute_limit: Some(1000),
        packets_per_minute_limit: Some(2),
        settlement_engine_url: Some("http://settlement.example".to_string()),
    });
    pub static ACCOUNT_DETAILS_1: Lazy<AccountDetails> = Lazy::new(|| AccountDetails {
        ilp_address: None,
        username: Username::from_str("bob").unwrap(),
        asset_scale: 9,
        asset_code: "ABC".to_string(),
        max_packet_amount: 1_000_000,
        min_balance: Some(0),
        ilp_over_http_url: Some("http://example.com/accounts/dylan/ilp".to_string()),
        // incoming token has is the account's username concatenated wiht the password
        ilp_over_http_incoming_token: Some(SecretString::new("incoming_auth_token".to_string())),
        ilp_over_http_outgoing_token: Some(SecretString::new("outgoing_auth_token".to_string())),
        ilp_over_btp_url: Some("btp+ws://example.com/accounts/dylan/ilp/btp".to_string()),
        ilp_over_btp_incoming_token: Some(SecretString::new("other_btp_token".to_string())),
        ilp_over_btp_outgoing_token: Some(SecretString::new("btp_token".to_string())),
        settle_threshold: Some(0),
        settle_to: Some(-1000),
        routing_relation: Some("Child".to_owned()),
        round_trip_time: None,
        amount_per_minute_limit: Some(1000),
        packets_per_minute_limit: Some(20),
        settlement_engine_url: None,
    });
    pub static ACCOUNT_DETAILS_2: Lazy<AccountDetails> = Lazy::new(|| AccountDetails {
        ilp_address: None,
        username: Username::from_str("charlie").unwrap(),
        asset_scale: 9,
        asset_code: "XRP".to_string(),
        max_packet_amount: 1000,
        min_balance: Some(0),
        ilp_over_http_url: None,
        ilp_over_http_incoming_token: None,
        ilp_over_http_outgoing_token: None,
        ilp_over_btp_url: None,
        ilp_over_btp_incoming_token: None,
        ilp_over_btp_outgoing_token: None,
        settle_threshold: Some(0),
        settle_to: None,
        routing_relation: None,
        round_trip_time: None,
        amount_per_minute_limit: None,
        packets_per_minute_limit: None,
        settlement_engine_url: None,
    });
}

mod redis_helpers {
    // Copied from https://github.com/mitsuhiko/redis-rs/blob/9a1777e8a90c82c315a481cdf66beb7d69e681a2/tests/support/mod.rs
    #![allow(dead_code)]

    use redis_crate::{self, RedisError};
    use socket2::{Domain, Socket, Type};
    use std::env;
    use std::fs;
    use std::net::SocketAddr;
    use std::path::PathBuf;
    use std::process;
    use std::thread::sleep;
    use std::time::Duration;

    #[derive(PartialEq)]
    enum ServerType {
        Tcp,
        Unix,
    }

    pub struct RedisServer {
        pub process: process::Child,
        addr: redis_crate::ConnectionAddr,
    }

    impl ServerType {
        fn get_intended() -> ServerType {
            match env::var("REDISRS_SERVER_TYPE")
                .ok()
                .as_ref()
                .map(|x| &x[..])
            {
                // Default to unix unlike original version
                Some("tcp") => ServerType::Tcp,
                _ => ServerType::Unix,
            }
        }
    }

    impl RedisServer {
        pub fn new() -> RedisServer {
            let server_type = ServerType::get_intended();
            let mut cmd = process::Command::new("redis-server");

            let fname = if os_type::current_platform().os_type == os_type::OSType::OSX {
                "libredis_cell.dylib"
            } else {
                "libredis_cell.so"
            };

            // Load redis_cell
            let mut cell_module: PathBuf = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
            cell_module.push("external");
            cell_module.push(fname);
            cmd.arg("--loadmodule").arg(cell_module.as_os_str());

            cmd.stdout(process::Stdio::null())
                .stderr(process::Stdio::null());

            let addr = match server_type {
                ServerType::Tcp => {
                    // this is technically a race but we can't do better with
                    // the tools that redis gives us :(
                    let socket = Socket::new(Domain::IPV4, Type::STREAM, None).unwrap();
                    socket.reuse_address().unwrap();
                    socket
                        .bind(&"127.0.0.1:0".parse::<SocketAddr>().unwrap().into())
                        .unwrap();
                    socket.listen(1).unwrap();
                    let listener = std::net::TcpListener::from(socket);
                    let server_port = listener.local_addr().unwrap().port();
                    cmd.arg("--port")
                        .arg(server_port.to_string())
                        .arg("--bind")
                        .arg("127.0.0.1");
                    redis_crate::ConnectionAddr::Tcp("127.0.0.1".to_string(), server_port)
                }
                ServerType::Unix => {
                    let (a, b) = rand::random::<(u64, u64)>();
                    let path = format!("/tmp/redis-rs-test-{}-{}.sock", a, b);
                    cmd.arg("--port").arg("0").arg("--unixsocket").arg(&path);
                    redis_crate::ConnectionAddr::Unix(PathBuf::from(&path))
                }
            };

            let process = cmd.spawn().unwrap();
            RedisServer { process, addr }
        }

        pub fn wait(&mut self) {
            self.process.wait().unwrap();
        }

        pub fn get_client_addr(&self) -> &redis_crate::ConnectionAddr {
            &self.addr
        }

        pub fn stop(&mut self) {
            let _ = self.process.kill();
            let _ = self.process.wait();
            if let redis_crate::ConnectionAddr::Unix(ref path) = *self.get_client_addr() {
                fs::remove_file(&path).ok();
            }
        }
    }

    impl Drop for RedisServer {
        fn drop(&mut self) {
            self.stop()
        }
    }

    pub struct TestContext {
        pub server: RedisServer,
        pub client: redis_crate::Client,
    }

    impl TestContext {
        pub fn new() -> TestContext {
            let server = RedisServer::new();

            let client = redis_crate::Client::open(redis_crate::ConnectionInfo {
                addr: server.get_client_addr().clone(),
                redis: redis_crate::RedisConnectionInfo {
                    db: 0,
                    username: None,
                    password: None,
                },
            })
            .unwrap();
            let mut con;

            let millisecond = Duration::from_millis(1);
            loop {
                match client.get_connection() {
                    Err(err) => {
                        if err.is_connection_refusal() {
                            sleep(millisecond);
                        } else {
                            panic!("Could not connect: {}", err);
                        }
                    }
                    Ok(x) => {
                        con = x;
                        break;
                    }
                }
            }
            redis_crate::cmd("FLUSHDB").execute(&mut con);

            TestContext { server, client }
        }

        // This one was added and not in the original file
        pub fn get_client_connection_info(&self) -> redis_crate::ConnectionInfo {
            redis_crate::ConnectionInfo {
                addr: self.server.get_client_addr().clone(),
                redis: redis_crate::RedisConnectionInfo {
                    db: 0,
                    username: None,
                    password: None,
                },
            }
        }

        pub fn connection(&self) -> redis_crate::Connection {
            self.client.get_connection().unwrap()
        }

        pub async fn async_connection(&self) -> Result<redis_crate::aio::Connection, ()> {
            Ok(self.client.get_async_connection().await.unwrap())
        }

        pub fn stop_server(&mut self) {
            self.server.stop();
        }

        pub async fn shared_async_connection(
            &self,
        ) -> Result<redis_crate::aio::MultiplexedConnection, RedisError> {
            self.client.get_multiplexed_tokio_connection().await
        }
    }
}

mod store_helpers {
    use super::fixtures::*;
    use super::redis_helpers::*;

    use interledger_api::NodeStore;
    use interledger_packet::Address;
    use interledger_service::{Account as AccountTrait, AddressStore};
    use interledger_store::{
        account::Account,
        redis::{RedisStore, RedisStoreBuilder},
    };
    use std::str::FromStr;

    pub async fn test_store() -> Result<(RedisStore, TestContext, Vec<Account>), ()> {
        let context = TestContext::new();
        let store = RedisStoreBuilder::new(context.get_client_connection_info(), [0; 32])
            .node_ilp_address(Address::from_str("example.node").unwrap())
            .connect()
            .await
            .unwrap();
        let mut accs = Vec::new();
        let acc = store
            .insert_account(ACCOUNT_DETAILS_0.clone())
            .await
            .unwrap();
        accs.push(acc.clone());
        // alice is a Parent, so the store's ilp address is updated to
        // the value that would be received by the ILDCP request. here,
        // we just assume alice appended some data to her address
        store
            .set_ilp_address(acc.ilp_address().with_suffix(b"user1").unwrap())
            .await
            .unwrap();

        let acc = store
            .insert_account(ACCOUNT_DETAILS_1.clone())
            .await
            .unwrap();
        accs.push(acc);
        Ok((store, context, accs))
    }
}
