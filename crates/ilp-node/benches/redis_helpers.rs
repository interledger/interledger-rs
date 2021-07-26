// Copied from https://github.com/mitsuhiko/redis-rs/blob/9a1777e8a90c82c315a481cdf66beb7d69e681a2/tests/support/mod.rs
#![allow(dead_code)]

use redis_crate::{self as redis, ConnectionAddr, ConnectionInfo, RedisError};
use socket2::{Domain, Socket, Type};
use std::env;
use std::fs;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::process;
use std::thread::sleep;
use std::time::Duration;

#[allow(unused)]
pub fn connection_info_to_string(info: ConnectionInfo) -> String {
    match info.addr {
        ConnectionAddr::Tcp(url, port) => format!("redis://{}:{}/{}", url, port, info.redis.db),
        ConnectionAddr::Unix(path) => {
            format!("redis+unix:{}?db={}", path.to_str().unwrap(), info.redis.db)
        }
        // FIXME: no idea what this should look like..
        ConnectionAddr::TcpTls { host, port, insecure } => todo!(),
    }
}

pub fn get_open_port(try_port: Option<u16>) -> u16 {
    if let Some(port) = try_port {
        let socket = Socket::new(Domain::ipv4(), Type::stream(), None).unwrap();
        socket.reuse_address().unwrap();
        if socket
            .bind(
                &format!("127.0.0.1:{}", port)
                    .parse::<SocketAddr>()
                    .unwrap()
                    .into(),
            )
            .is_ok()
        {
            socket.listen(1).unwrap();
            let listener = socket.into_tcp_listener();
            return listener.local_addr().unwrap().port();
        }
    }

    for _i in 0..1000 {
        let socket = Socket::new(Domain::ipv4(), Type::stream(), None).unwrap();
        socket.reuse_address().unwrap();
        if socket
            .bind(&"127.0.0.1:0".parse::<SocketAddr>().unwrap().into())
            .is_ok()
        {
            socket.listen(1).unwrap();
            let listener = socket.into_tcp_listener();
            return listener.local_addr().unwrap().port();
        }
    }

    panic!("Cannot find open port!");
}

pub async fn delay(ms: u64) {
    tokio::time::delay_for(Duration::from_millis(ms)).await;
}

#[derive(PartialEq)]
enum ServerType {
    Tcp,
    Unix,
}

pub struct RedisServer {
    pub process: process::Child,
    addr: redis::ConnectionAddr,
}

impl ServerType {
    fn get_intended() -> ServerType {
        match env::var("REDISRS_SERVER_TYPE")
            .ok()
            .as_ref()
            .map(|x| &x[..])
        {
            // Default to unix socket unlike original version
            Some("tcp") => ServerType::Tcp,
            _ => ServerType::Unix,
        }
    }
}

impl RedisServer {
    pub fn new() -> RedisServer {
        let server_type = ServerType::get_intended();
        let mut cmd = process::Command::new("redis-server");
        cmd.stdout(process::Stdio::null())
            .stderr(process::Stdio::null());

        let addr = match server_type {
            ServerType::Tcp => {
                // this is technically a race but we can't do better with
                // the tools that redis gives us :(
                let socket = Socket::new(Domain::ipv4(), Type::stream(), None).unwrap();
                socket.reuse_address().unwrap();
                socket
                    .bind(&"127.0.0.1:0".parse::<SocketAddr>().unwrap().into())
                    .unwrap();
                socket.listen(1).unwrap();
                let listener = socket.into_tcp_listener();
                let server_port = listener.local_addr().unwrap().port();
                cmd.arg("--port")
                    .arg(server_port.to_string())
                    .arg("--bind")
                    .arg("127.0.0.1");
                redis::ConnectionAddr::Tcp("127.0.0.1".to_string(), server_port)
            }
            ServerType::Unix => {
                let (a, b) = rand::random::<(u64, u64)>();
                let path = format!("/tmp/redis-rs-test-{}-{}.sock", a, b);
                cmd.arg("--port").arg("0").arg("--unixsocket").arg(&path);
                redis::ConnectionAddr::Unix(PathBuf::from(&path))
            }
        };

        let process = cmd.spawn().expect(
            "Could not spawn redis-server process, please ensure \
             that all redis components are installed",
        );
        RedisServer { process, addr }
    }

    pub fn wait(&mut self) {
        self.process.wait().unwrap();
    }

    pub fn get_client_addr(&self) -> &redis::ConnectionAddr {
        &self.addr
    }

    pub fn stop(&mut self) {
        let _ = self.process.kill();
        let _ = self.process.wait();
        if let redis::ConnectionAddr::Unix(ref path) = *self.get_client_addr() {
            fs::remove_file(&path).ok();
        }
    }
}

impl Default for RedisServer {
    fn default() -> Self {
        RedisServer::new()
    }
}

impl Drop for RedisServer {
    fn drop(&mut self) {
        self.stop()
    }
}

pub struct TestContext {
    pub server: RedisServer,
    pub client: redis::Client,
}

impl TestContext {
    pub fn new() -> TestContext {
        let server = RedisServer::new();

        let client = redis::Client::open(redis::ConnectionInfo {
            addr: server.get_client_addr().clone(),
            redis: redis::RedisConnectionInfo {
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
        redis::cmd("FLUSHALL").execute(&mut con);

        TestContext { server, client }
    }

    // This one was added and not in the original file
    pub fn get_client_connection_info(&self) -> redis::ConnectionInfo {
        redis::ConnectionInfo {
            addr: self.server.get_client_addr().clone(),
            redis: redis::RedisConnectionInfo {
                db: 0,
                username: None,
                password: None,
            },
        }
    }

    pub fn connection(&self) -> redis::Connection {
        self.client.get_connection().unwrap()
    }

    pub async fn async_connection(&self) -> Result<redis::aio::Connection, ()> {
        Ok(self.client.get_async_connection().await.unwrap())
    }

    pub fn stop_server(&mut self) {
        self.server.stop();
    }

    pub async fn shared_async_connection(
        &self,
    ) -> Result<redis::aio::MultiplexedConnection, RedisError> {
        self.client.get_multiplexed_tokio_connection().await
    }
}

impl Default for TestContext {
    fn default() -> Self {
        TestContext::new()
    }
}
