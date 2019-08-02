#![recursion_limit = "128"]

use env_logger;
use futures::{Future, Stream};
use interledger::{
    cli,
    node::{AccountDetails, InterledgerNode},
};
use interledger_packet::Address;
use serde_json::json;
use std::str;
use std::str::FromStr;
use std::thread::sleep;
use std::time::Duration;
use tokio::runtime::Builder as RuntimeBuilder;

mod redis_helpers;
use redis_helpers::*;
use std::process::Command;

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
use interledger_settlement_engines::engines::ethereum_ledger::run_ethereum_engine;

#[derive(serde::Deserialize)]
struct DeliveryData {
    delivered_amount: u64,
}

#[test]
/// In this test we have Alice and Bob who have peered with each other and run
/// Ethereum ledger settlement engines. Alice proceeds to make SPSP payments to
/// Bob, until she eventually reaches Bob's `settle_threshold`. Once that's
/// exceeded, her engine makes a settlement request to Bob. Alice's connector
/// immediately applies the balance change. Bob's engine listens for incoming
/// transactions, and once the transaction has sufficient confirmations it
/// lets Bob's connector know about it, so that it adjusts their credit.
fn eth_ledger_settlement() {
    // Nodes 1 and 2 are peers, Node 2 is the parent of Node 3
    let _ = env_logger::try_init();
    let context = TestContext::new();

    let mut ganache_pid = start_ganache();

    // Each node will use its own DB within the redis instance
    let mut connection_info1 = context.get_client_connection_info();
    connection_info1.db = 1;
    let mut connection_info2 = context.get_client_connection_info();
    connection_info2.db = 2;

    let node1_http = get_open_port(Some(3010));
    let node1_settlement = get_open_port(Some(3011));
    let node1_engine = get_open_port(Some(3012));
    let alice_key = "380eb0f3d505f087e438eca80bc4df9a7faa24f868e69fc0440261a0fc0567dc".to_string();
    let node2_http = get_open_port(Some(3020));
    let node2_settlement = get_open_port(Some(3021));
    let node2_engine = get_open_port(Some(3022));
    let bob_key = "cc96601bc52293b53c4736a12af9130abf347669b3813f9ec4cafdf6991b087e".to_string();

    let mut runtime = RuntimeBuilder::new()
        .panic_handler(|_| panic!("Tokio worker panicked"))
        .build()
        .unwrap();

    let node1_secret = cli::random_secret();
    let node1 = InterledgerNode {
        ilp_address: Address::from_str("example.alice").unwrap(),
        default_spsp_account: Some(0),
        admin_auth_token: "hi_alice".to_string(),
        redis_connection: connection_info1.clone(),
        btp_address: ([127, 0, 0, 1], get_open_port(None)).into(),
        http_address: ([127, 0, 0, 1], node1_http).into(),
        settlement_address: ([127, 0, 0, 1], node1_settlement).into(),
        secret_seed: node1_secret,
        route_broadcast_interval: Some(200),
    };
    let node1_clone = node1.clone();
    runtime.spawn(
        run_ethereum_engine(
            connection_info1,
            "http://localhost:8545".to_string(),
            node1_engine,
            &node1_secret,
            alice_key,
            1,
            0,
            18,
            1000,
            format!("http://127.0.0.1:{}", node1_settlement),
            None,
            true,
        )
        .and_then(move |_| {
            // TODO insert the accounts via HTTP request
            node1_clone
                .insert_account(AccountDetails {
                    ilp_address: Address::from_str("example.alice").unwrap(),
                    asset_code: "ETH".to_string(),
                    asset_scale: 18,
                    btp_incoming_token: None,
                    btp_uri: None,
                    http_endpoint: None,
                    http_incoming_token: Some("in_alice".to_string()),
                    http_outgoing_token: Some("out_alice".to_string()),
                    max_packet_amount: 10,
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
                .and_then(move |_| {
                    node1_clone.insert_account(AccountDetails {
                        ilp_address: Address::from_str("example.bob").unwrap(),
                        asset_code: "ETH".to_string(),
                        asset_scale: 18,
                        btp_incoming_token: None,
                        btp_uri: None,
                        http_endpoint: Some(format!("http://localhost:{}/ilp", node2_http)),
                        http_incoming_token: Some("bob".to_string()),
                        http_outgoing_token: Some("alice".to_string()),
                        max_packet_amount: 10,
                        min_balance: Some(-100),
                        settle_threshold: Some(70),
                        settle_to: Some(10),
                        send_routes: false,
                        receive_routes: false,
                        routing_relation: None,
                        round_trip_time: None,
                        packets_per_minute_limit: None,
                        amount_per_minute_limit: None,
                        settlement_engine_url: Some(format!("http://localhost:{}", node1_engine)),
                        settlement_engine_asset_scale: Some(18),
                    })
                })
                .and_then(move |_| node1.serve())
        }),
    );

    let node2_secret = cli::random_secret();
    let node2 = InterledgerNode {
        ilp_address: Address::from_str("example.bob").unwrap(),
        default_spsp_account: Some(0),
        admin_auth_token: "admin".to_string(),
        redis_connection: connection_info2.clone(),
        btp_address: ([127, 0, 0, 1], get_open_port(None)).into(),
        http_address: ([127, 0, 0, 1], node2_http).into(),
        settlement_address: ([127, 0, 0, 1], node2_settlement).into(),
        secret_seed: node2_secret,
        route_broadcast_interval: Some(200),
    };
    runtime.spawn(
        run_ethereum_engine(
            connection_info2,
            "http://localhost:8545".to_string(),
            node2_engine,
            &node2_secret,
            bob_key,
            1,
            0,
            18,
            1000,
            format!("http://127.0.0.1:{}", node2_settlement),
            None,
            true,
        )
        .and_then(move |_| {
            node2
                .insert_account(AccountDetails {
                    ilp_address: Address::from_str("example.bob").unwrap(),
                    asset_code: "ETH".to_string(),
                    asset_scale: 18,
                    btp_incoming_token: None,
                    btp_uri: None,
                    http_endpoint: None,
                    http_incoming_token: Some("in_bob".to_string()),
                    http_outgoing_token: Some("out_bob".to_string()),
                    max_packet_amount: 10,
                    min_balance: None,
                    settle_threshold: None,
                    settle_to: None,
                    send_routes: false,
                    receive_routes: false,
                    routing_relation: None,
                    round_trip_time: None,
                    packets_per_minute_limit: None,
                    amount_per_minute_limit: None,
                    settlement_engine_url: None,
                    settlement_engine_asset_scale: None,
                })
                .and_then(move |_| {
                    node2
                        .insert_account(AccountDetails {
                            ilp_address: Address::from_str("example.alice").unwrap(),
                            asset_code: "ETH".to_string(),
                            asset_scale: 18,
                            btp_incoming_token: None,
                            btp_uri: None,
                            http_endpoint: Some(format!("http://localhost:{}/ilp", node1_http)),
                            http_incoming_token: Some("alice".to_string()),
                            http_outgoing_token: Some("bob".to_string()),
                            max_packet_amount: 10,
                            min_balance: Some(-100),
                            settle_threshold: Some(70),
                            settle_to: Some(-10),
                            send_routes: false,
                            receive_routes: false,
                            routing_relation: None,
                            round_trip_time: None,
                            packets_per_minute_limit: None,
                            amount_per_minute_limit: None,
                            settlement_engine_url: Some(format!(
                                "http://localhost:{}",
                                node2_engine
                            )),
                            settlement_engine_asset_scale: Some(18),
                        })
                        .and_then(move |_| node2.serve())
                })
        }),
    );

    runtime
        .block_on(
            // Wait for the nodes to spin up
            delay(500)
                .map_err(|_| panic!("Something strange happened"))
                .and_then(move |_| {
                    // The 2 nodes are peered, we make a POST to the engine's
                    // create account endpoint so that they trade addresses.
                    // This would happen automatically if we inserted the
                    // accounts via the Accounts API.
                    let client = reqwest::r#async::Client::new();

                    let create_account = |engine_port, account_id| {
                        client
                            .post(&format!("http://localhost:{}/accounts", engine_port))
                            .json(&json!({ "id": account_id }))
                            .send()
                            .map_err(|err| {
                                eprintln!("Error creating account: {:?}", err);
                                err
                            })
                            .and_then(|res| res.error_for_status())
                    };

                    let send_money = |from, to, amount| {
                        client
                            .post(&format!("http://localhost:{}/pay", from))
                            .header("Authorization", "Bearer in_alice")
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

                    let create1 = create_account(node1_engine, "1");
                    let create2 = create_account(node2_engine, "1");

                    // Make 4 subsequent payments (we could also do a 71 payment
                    // directly)
                    let send1 = send_money(node1_http, node2_http, 10);
                    let send2 = send_money(node1_http, node2_http, 20);
                    let send3 = send_money(node1_http, node2_http, 40);
                    let send4 = send_money(node1_http, node2_http, 1);

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

                    create1
                        .and_then(move |_| create2)
                        .and_then(move |_| send1)
                        .and_then(move |_| {
                            get_balance(1, node1_http, "bob").and_then(move |ret| {
                                let ret = str::from_utf8(&ret).unwrap();
                                assert_eq!(ret, "{\"balance\":\"10\"}");
                                get_balance(1, node2_http, "alice").and_then(move |ret| {
                                    let ret = str::from_utf8(&ret).unwrap();
                                    assert_eq!(ret, "{\"balance\":\"-10\"}");
                                    Ok(())
                                })
                            })
                        })
                        .and_then(move |_| send2)
                        .and_then(move |_| {
                            get_balance(1, node1_http, "bob").and_then(move |ret| {
                                let ret = str::from_utf8(&ret).unwrap();
                                assert_eq!(ret, "{\"balance\":\"30\"}");
                                get_balance(1, node2_http, "alice").and_then(move |ret| {
                                    let ret = str::from_utf8(&ret).unwrap();
                                    assert_eq!(ret, "{\"balance\":\"-30\"}");
                                    Ok(())
                                })
                            })
                        })
                        .and_then(move |_| send3)
                        .and_then(move |_| {
                            get_balance(1, node1_http, "bob").and_then(move |ret| {
                                let ret = str::from_utf8(&ret).unwrap();
                                assert_eq!(ret, "{\"balance\":\"70\"}");
                                get_balance(1, node2_http, "alice").and_then(move |ret| {
                                    let ret = str::from_utf8(&ret).unwrap();
                                    assert_eq!(ret, "{\"balance\":\"-70\"}");
                                    Ok(())
                                })
                            })
                        })
                        // Up to here, Alice's balance should be -70 and Bob's
                        // balance should be 70. Once we make 1 more payment, we
                        // exceed the settle_threshold and thus a settlement is made
                        .and_then(move |_| send4)
                        .and_then(move |_| {
                            // Wait a few seconds so that the receiver's engine
                            // gets the data
                            sleep(Duration::from_secs(5));
                            // Since the credit connection reached -71, and the
                            // settle_to is -10, a 61 Wei transaction is made.
                            get_balance(1, node1_http, "bob").and_then(move |ret| {
                                let ret = str::from_utf8(&ret).unwrap();
                                assert_eq!(ret, "{\"balance\":\"10\"}");
                                get_balance(1, node2_http, "alice").and_then(move |ret| {
                                    let ret = str::from_utf8(&ret).unwrap();
                                    assert_eq!(ret, "{\"balance\":\"-10\"}");
                                    ganache_pid.kill().unwrap();
                                    Ok(())
                                })
                            })
                        })
                }),
        )
        .unwrap();
}
