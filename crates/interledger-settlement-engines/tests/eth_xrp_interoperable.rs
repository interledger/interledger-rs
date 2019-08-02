#![recursion_limit = "128"]

use env_logger;
use futures::{future::join_all, Future, Stream};
use interledger::{
    cli,
    node::{AccountDetails, InterledgerNode},
};
use interledger_packet::Address;
use serde_json::json;
use std::str;
use std::str::FromStr;
use tokio::runtime::Builder as RuntimeBuilder;
use std::time::Duration;
use std::thread::sleep;

mod redis_helpers;
use redis_helpers::*;
use std::process::Command;
use interledger_settlement_engines::engines::ethereum_ledger::run_ethereum_engine;

#[derive(serde::Deserialize)]
struct DeliveryData {
    delivered_amount: u64,
}

fn start_ganache() -> std::process::Child {
    let mut ganache = Command::new("ganache-cli");
    let ganache = ganache.stdout(std::process::Stdio::null()).arg("-m").arg(
        "abstract vacuum mammal awkward pudding scene penalty purchase dinner depart evoke puzzle",
    );
    let ganache_pid = ganache.spawn().expect("couldnt start ganache-cli");
    // wait a couple of seconds for ganache to boot up
    sleep(Duration::from_secs(5));
    ganache_pid
}

fn start_xrp_engine(
    connector_url: &str,
    redis_port: u16,
    engine_port: u16,
    xrp_address: &str,
    xrp_secret: &str,
) -> std::process::Child {
    let mut engine = Command::new("ilp-settlement-xrp");
    engine
        .env("DEBUG", "ilp-settlement-xrp")
        .env("CONNECTOR_URL", connector_url)
        .env("REDIS_PORT", redis_port.to_string())
        .env("ENGINE_PORT", engine_port.to_string())
        .env("LEDGER_ADDRESS", xrp_address)
        .env("LEDGER_SECRET", xrp_secret);
    let engine_pid = engine
        // .stderr(std::process::Stdio::null())
        // .stdout(std::process::Stdio::null())
        .spawn()
        .expect("couldnt start xrp engine");
    sleep(Duration::from_secs(2));
    engine_pid
}


#[test]
fn eth_xrp_interoperable() {
    // Nodes 1 and 2 are peers, Node 2 is the parent of Node 3
    let _ = env_logger::try_init();
    let context = TestContext::new();

    let mut ganache_pid = start_ganache();

    // Each node will use its own DB within the redis instance
    let mut connection_info1 = context.get_client_connection_info();
    connection_info1.db = 1;
    let mut connection_info2 = context.get_client_connection_info();
    connection_info2.db = 2;
    let mut connection_info3 = context.get_client_connection_info();
    connection_info3.db = 3;


    let node1_http = get_open_port(Some(3010));
    let node1_settlement = get_open_port(Some(3011));
    let node1_engine = get_open_port(Some(3012));

    let node2_http = get_open_port(Some(3020));
    let node2_settlement = get_open_port(Some(3021));
    let node2_engine = get_open_port(Some(3022));
    let node2_xrp_engine_port = get_open_port(Some(3023));
    let node2_btp = get_open_port(Some(3024));

    let node3_http = get_open_port(Some(3030));
    let node3_settlement = get_open_port(Some(3031));
    let node3_engine = get_open_port(Some(3032));
    let node3_xrp_engine_port = get_open_port(Some(3033));

    // spawn 2 redis servers for the XRP engines
    let node2_redis_port = get_open_port(Some(6379));
    let node3_redis_port = get_open_port(Some(6380));
    let mut node2_engine_redis = RedisServer::spawn_with_port(node2_redis_port);
    let mut node3_engine_redis = RedisServer::spawn_with_port(node3_redis_port);
    let mut node2_xrp_engine = start_xrp_engine(
        &format!("http://localhost:{}", node2_http),
        node2_redis_port,
        node2_xrp_engine_port,
        "rGCUgMH4omQV1PUuYFoMAnA7esWFhE7ZEV",
        "sahVoeg97nuitefnzL9GHjp2Z6kpj",
    );
    let mut node3_xrp_engine = start_xrp_engine(
        &format!("http://localhost:{}", node3_http),
        node3_redis_port,
        node3_xrp_engine_port,
        "r3GDnYaYCk2XKzEDNYj59yMqDZ7zGih94K",
        "ssnYUDNeNQrNij2EVJG6dDw258jA6",
    );

    let node1_eth_key = "380eb0f3d505f087e438eca80bc4df9a7faa24f868e69fc0440261a0fc0567dc".to_string();
    let node2_eth_key = "cc96601bc52293b53c4736a12af9130abf347669b3813f9ec4cafdf6991b087e".to_string();
    let eth_asset_scale = 6;
    let xrp_asset_scale = 6;
    let poll_frequency = 1000; // ms
    let confirmations = 0; // set this to something higher in production
    let chain_id = 1;
    let ganache_url = "http://localhost:8545".to_string();
    let node1_eth_engine_fut = run_ethereum_engine(
        connection_info1.clone(),
        ganache_url.clone(),
        node1_engine,
        &cli::random_secret(),
        node1_eth_key,
        chain_id,
        confirmations,
        eth_asset_scale,
        poll_frequency,
        format!("http://127.0.0.1:{}", node1_settlement),
        None,
        true,
    );

    let node2_eth_engine_fut = run_ethereum_engine(
        connection_info2.clone(),
        ganache_url.clone(),
        node2_engine,
        &cli::random_secret(),
        node2_eth_key,
        chain_id,
        confirmations,
        eth_asset_scale,
        poll_frequency,
        format!("http://127.0.0.1:{}", node2_settlement),
        None,
        true,
    );

    let mut runtime = RuntimeBuilder::new()
        .panic_handler(|_| panic!("Tokio worker panicked"))
        .build()
        .unwrap();

    let node1 = InterledgerNode {
        ilp_address: Address::from_str("example.alice").unwrap(),
        default_spsp_account: Some(0),
        admin_auth_token: "admin".to_string(),
        redis_connection: connection_info1,
        btp_address: ([127, 0, 0, 1], get_open_port(None)).into(),
        http_address: ([127, 0, 0, 1], node1_http).into(),
        settlement_address: ([127, 0, 0, 1], node1_settlement).into(),
        secret_seed: cli::random_secret(),
        route_broadcast_interval: Some(200),
    };
    let node1_clone = node1.clone();
    runtime.spawn(
        // TODO insert the accounts via HTTP request
        node1_eth_engine_fut
        .and_then(move |_| {
            node1_clone
                .insert_account(AccountDetails {
                    ilp_address: Address::from_str("example.alice").unwrap(),
                    asset_code: "ETH".to_string(),
                    asset_scale: eth_asset_scale,
                    btp_incoming_token: None,
                    btp_uri: None,
                    http_endpoint: Some(format!("http://localhost:{}/ilp", node1_http)),
                    http_incoming_token: Some("in_alice".to_string()),
                    http_outgoing_token: Some("out_alice".to_string()),
                    max_packet_amount: u64::max_value(),
                    min_balance: None,
                    settle_threshold: None,
                    settle_to: Some(-10),
                    send_routes: false,
                    receive_routes: false,
                    routing_relation: None,
                    round_trip_time: None,
                    packets_per_minute_limit: None,
                    amount_per_minute_limit: None,
                    settlement_engine_url: None,
                    settlement_engine_asset_scale: None,
                })
                .and_then(move |_|
            // TODO insert the accounts via HTTP request
            node1_clone
                .insert_account(AccountDetails {
                    ilp_address: Address::from_str("example.bob").unwrap(),
                    asset_code: "ETH".to_string(),
                    asset_scale: eth_asset_scale,
                    btp_incoming_token: None,
                    btp_uri: None,
                    http_endpoint: Some(format!("http://localhost:{}/ilp", node2_http)),
                    http_incoming_token: Some("bob".to_string()),
                    http_outgoing_token: Some("alice".to_string()),
                    max_packet_amount: u64::max_value(),
                    min_balance: Some(-100),
                    settle_threshold: Some(70),
                    settle_to: Some(-10),
                    send_routes: true,
                    receive_routes: true,
                    routing_relation: Some("Peer".to_string()),
                    round_trip_time: None,
                    packets_per_minute_limit: None,
                    amount_per_minute_limit: None,
                    settlement_engine_url: Some(format!("http://localhost:{}", node1_engine)),
                    settlement_engine_asset_scale: Some(18),
                }))
                .and_then(move |_| node1.serve())
        })
    );

    let node2 = InterledgerNode {
        ilp_address: Address::from_str("example.bob").unwrap(),
        default_spsp_account: Some(0),
        admin_auth_token: "admin".to_string(),
        redis_connection: connection_info2,
        btp_address: ([127, 0, 0, 1], node2_btp).into(),
        http_address: ([127, 0, 0, 1], node2_http).into(),
        settlement_address: ([127, 0, 0, 1], node2_settlement).into(),
        secret_seed: cli::random_secret(),
        route_broadcast_interval: Some(200),
    };
    let node2_clone = node2.clone();
    runtime.spawn(
        node2_eth_engine_fut.and_then(move |_| {
                node2_clone.insert_account(AccountDetails {
                    ilp_address: Address::from_str("example.alice").unwrap(),
                    asset_code: "ETH".to_string(),
                    asset_scale: eth_asset_scale,
                    btp_incoming_token: None,
                    btp_uri: None,
                    http_endpoint: Some(format!("http://localhost:{}/ilp", node1_http)),
                    http_incoming_token: Some("alice".to_string()),
                    http_outgoing_token: Some("bob".to_string()),
                    max_packet_amount: u64::max_value(),
                    min_balance: Some(-100),
                    settle_threshold: Some(70),
                    settle_to: Some(-10),
                    send_routes: true,
                    receive_routes: true,
                    routing_relation: Some("Peer".to_string()),
                    round_trip_time: None,
                    packets_per_minute_limit: None,
                    amount_per_minute_limit: None,
                    settlement_engine_url: Some(format!(
                        "http://localhost:{}",
                        node2_engine
                    )),
                    settlement_engine_asset_scale: Some(eth_asset_scale),
                })
                .and_then(move |_| {
                    node2_clone.insert_account(AccountDetails {
                        ilp_address: Address::from_str("example.bob.charlie").unwrap(),
                        asset_code: "XRP".to_string(),
                        asset_scale: xrp_asset_scale,
                        btp_incoming_token: None,
                        btp_uri: None,
                        http_endpoint: Some(format!("http://localhost:{}/ilp", node3_http)),
                        http_incoming_token: Some("charlie".to_string()),
                        http_outgoing_token: Some("bob".to_string()),
                        max_packet_amount: u64::max_value(),
                        min_balance: Some(-100),
                        settle_threshold: Some(70),
                        settle_to: Some(10),
                        send_routes: false,
                        receive_routes: true,
                        routing_relation: Some("Child".to_string()),
                        round_trip_time: None,
                        packets_per_minute_limit: None,
                        amount_per_minute_limit: None,
                        settlement_engine_url: Some(format!("http://localhost:{}", node2_xrp_engine_port)),
                        settlement_engine_asset_scale: Some(xrp_asset_scale),
                    })
                })
            })
            .and_then(move |_| node2.serve())
            .and_then(move |_| {
                let client = reqwest::r#async::Client::new();
                client
                    .put(&format!("http://localhost:{}/rates", node2_http))
                    .header("Authorization", "Bearer admin")
                    .json(&json!({"XRP": 1, "ETH": 1}))
                    .send()
                    .map_err(|err| panic!(err))
                    .and_then(|res| {
                        res.error_for_status()
                            .expect("Error setting exchange rates");
                        Ok(())
                    })
            })
    );

    let node3 = InterledgerNode {
        ilp_address: Address::from_str("example.bob.charlie").unwrap(),
        default_spsp_account: Some(0),
        admin_auth_token: "admin".to_string(),
        redis_connection: connection_info3,
        btp_address: ([127, 0, 0, 1], get_open_port(None)).into(),
        http_address: ([127, 0, 0, 1], node3_http).into(),
        settlement_address: ([127, 0, 0, 1], node3_settlement).into(),
        secret_seed: cli::random_secret(),
        route_broadcast_interval: Some(200),
    };
    let node3_clone = node3.clone();
    runtime.spawn(
        // Wait a bit to make sure the other node's BTP server is listening
        delay(50).map_err(|err| panic!(err)).and_then(move |_| {
                node3_clone.insert_account(AccountDetails {
                    ilp_address: Address::from_str("example.bob.charlie").unwrap(),
                    asset_code: "XRP".to_string(),
                    asset_scale: xrp_asset_scale,
                    btp_incoming_token: None,
                    btp_uri: None,
                    http_endpoint: Some(format!("http://localhost:{}/ilp", node3_http)),
                    http_incoming_token: Some("in_charlie".to_string()),
                    http_outgoing_token: Some("out_charlie".to_string()),
                    max_packet_amount: u64::max_value(),
                    min_balance: None,
                    settle_threshold: None,
                    settle_to: Some(-10),
                    send_routes: false,
                    receive_routes: false,
                    routing_relation: None,
                    round_trip_time: None,
                    packets_per_minute_limit: None,
                    amount_per_minute_limit: None,
                    settlement_engine_url: None,
                    settlement_engine_asset_scale: None,
                }).and_then(move |_| {
                    node3_clone.insert_account(AccountDetails {
                        ilp_address: Address::from_str("example.bob").unwrap(),
                        asset_code: "XRP".to_string(),
                        asset_scale: xrp_asset_scale,
                        btp_incoming_token: None,
                        btp_uri: None,
                        http_endpoint: Some(format!("http://localhost:{}/ilp", node2_http)),
                        http_incoming_token: Some("bob".to_string()),
                        http_outgoing_token: Some("charlie".to_string()),
                        max_packet_amount: u64::max_value(),
                        min_balance: Some(-100),
                        settle_threshold: Some(70),
                        settle_to: Some(10),
                        send_routes: true,
                        receive_routes: false,
                        routing_relation: Some("Parent".to_string()),
                        round_trip_time: None,
                        packets_per_minute_limit: None,
                        amount_per_minute_limit: None,
                        settlement_engine_url: Some(format!("http://localhost:{}", node3_xrp_engine_port)),
                        settlement_engine_asset_scale: Some(xrp_asset_scale),
                    })
                })
            })
            .and_then(move |_| node3.serve())
    );

    runtime
        .block_on(
            // Wait for the nodes to spin up
            delay(500)
                .map_err(|_| panic!("Something strange happened"))
                .and_then(move |_| {
                    let create_account = |engine_port, account_id| {
                        let client = reqwest::r#async::Client::new();
                        client
                            .post(&format!("http://localhost:{}/accounts", engine_port))
                            .header("Content-Type", "application/json")
                            .json(&json!({ "id": account_id }))
                            .send()
                            .map_err(|err| {
                                eprintln!("Error creating account: {:?}", err);
                                err
                            })
                            .and_then(|res| res.error_for_status())
                            .and_then(|res| res.into_body().concat2())
                            .and_then(move |res| { println!("GOT RES {} {} {:?}", engine_port, account_id, res); Ok(res)})
                    };

                    let send_money = |from, to, amount, auth| {
                        let client = reqwest::r#async::Client::new();
                        client
                            .post(&format!("http://localhost:{}/pay", from))
                            .header("Authorization", format!("Bearer {}", auth))
                            .json(&json!({
                                "receiver": format!("http://localhost:{}/.well-known/pay", to),
                                "source_amount": amount,
                            }))
                            .send()
                            .map_err(|err| {
                                eprintln!("Error sending SPSP payment: {:?}", err);
                                err
                            })
                            .and_then(|res| res.error_for_status())
                            .and_then(|res| res.into_body().concat2())
                            .and_then(move |body| {
                                let ret: DeliveryData = serde_json::from_slice(&body).unwrap();
                                assert_eq!(ret.delivered_amount, amount);
                                Ok(())
                            })
                    };

                    let get_balance = |account_id, node_port, admin_token| {
                        let client = reqwest::r#async::Client::new();
                        client
                            .get(&format!(
                                "http://localhost:{}/accounts/{}/balance",
                                node_port, account_id
                            ))
                            .header("Authorization", format!("Bearer {}", admin_token))
                            .send()
                            .map_err(|err| {
                                eprintln!("Error getting account data: {:?}", err);
                                err
                            })
                            .and_then(|res| res.error_for_status())
                            .and_then(|res| res.into_body().concat2())
                    };

                    // Insert accounts for the 3 nodes (4 total since node2 has
                    // eth & xrp)
                    create_account(node1_engine, "1")
                    .and_then(move |_| create_account(node2_engine, "0"))
                    .and_then(move |_| create_account(node2_xrp_engine_port, "1"))
                    .and_then(move |_| create_account(node3_xrp_engine_port, "1"))
                    .and_then(move |_| send_money(node1_http, node3_http, 70, "in_alice"))
                    .and_then(move |_| send_money(node1_http, node3_http, 1, "in_alice"))
                    .and_then(move |_| {
                        get_balance(0, node1_http, "admin").and_then(move |ret| {
                            let ret = str::from_utf8(&ret).unwrap();
                            assert_eq!(ret, "{\"balance\":\"-10\"}");
                            get_balance(0, node3_http, "admin").and_then(move |ret| {
                                let ret = str::from_utf8(&ret).unwrap();
                                assert_eq!(ret, "{\"balance\":\"10\"}");
                                node2_engine_redis.kill().unwrap();
                                node3_engine_redis.kill().unwrap();
                                node2_xrp_engine.kill().unwrap();
                                node3_xrp_engine.kill().unwrap();
                                ganache_pid.kill().unwrap();
                                Ok(())
                            })
                        })
                    })
                })
        )
        .unwrap();
}
