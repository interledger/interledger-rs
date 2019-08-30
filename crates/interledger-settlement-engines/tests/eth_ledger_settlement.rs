#![recursion_limit = "128"]

use env_logger;
use futures::future::join_all;
use futures::Future;
use interledger::{
    cli,
    node::{AccountDetails, InterledgerNode},
};
use interledger_packet::Address;
use interledger_service::Username;
use std::net::SocketAddr;
use std::str::FromStr;
use tokio::runtime::Builder as RuntimeBuilder;

mod redis_helpers;
use redis_helpers::*;

mod test_helpers;
use test_helpers::{
    accounts_to_ids, create_account_on_engine, get_all_accounts, get_balance,
    send_money_to_username, start_eth_engine, start_ganache,
};

#[test]
/// In this test we have Alice and Bob who have peered with each other and run
/// Ethereum ledger settlement engines. Alice proceeds to make SPSP payments to
/// Bob, until she eventually reaches Bob's `settle_threshold`. Once that's
/// exceeded, her engine makes a settlement request to Bob. Alice's connector
/// immediately applies the balance change. Bob's engine listens for incoming
/// transactions, and once the transaction has sufficient confirmations it
/// lets Bob's connector know about it, so that it adjusts their credit.
fn eth_ledger_settlement() {
    let eth_decimals = 9;
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
    let node1_engine_address = SocketAddr::from(([127, 0, 0, 1], node1_engine));
    let alice_key = "380eb0f3d505f087e438eca80bc4df9a7faa24f868e69fc0440261a0fc0567dc".to_string();
    let node2_http = get_open_port(Some(3020));
    let node2_settlement = get_open_port(Some(3021));
    let node2_engine = get_open_port(Some(3022));
    let node2_engine_address = SocketAddr::from(([127, 0, 0, 1], node2_engine));
    let bob_key = "cc96601bc52293b53c4736a12af9130abf347669b3813f9ec4cafdf6991b087e".to_string();

    let mut runtime = RuntimeBuilder::new()
        .panic_handler(|_| panic!("Tokio worker panicked"))
        .build()
        .unwrap();

    let node1_secret = cli::random_secret();
    let node1 = InterledgerNode {
        ilp_address: Address::from_str("example.alice").unwrap(),
        default_spsp_account: None,
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
        start_eth_engine(connection_info1, node1_engine_address, alice_key, node1_settlement).and_then(
            move |_| {
                // TODO insert the accounts via HTTP request
                node1_clone
                    .insert_account(AccountDetails {
                        ilp_address: Address::from_str("example.alice").unwrap(),
                        username: Username::from_str("alice").unwrap(),
                        asset_code: "ETH".to_string(),
                        asset_scale: eth_decimals,
                        btp_incoming_token: None,
                        btp_uri: None,
                        http_endpoint: None,
                        http_incoming_token: Some("in_alice".to_string()),
                        http_outgoing_token: None,
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
                    })
                    .and_then(move |_| {
                        node1_clone.insert_account(AccountDetails {
                            ilp_address: Address::from_str("example.bob").unwrap(),
                            username: Username::from_str("bob").unwrap(),
                            asset_code: "ETH".to_string(),
                            asset_scale: eth_decimals,
                            btp_incoming_token: None,
                            btp_uri: None,
                            http_endpoint: Some(format!("http://localhost:{}/ilp", node2_http)),
                            http_incoming_token: Some("alice".to_string()),
                            http_outgoing_token: Some("alice:bob".to_string()),
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
                            settlement_engine_url: Some(format!(
                                "http://localhost:{}",
                                node1_engine
                            )),
                        })
                    })
                    .and_then(move |_| node1.serve())
            },
        ),
    );

    let node2_secret = cli::random_secret();
    let node2 = InterledgerNode {
        ilp_address: Address::from_str("example.bob").unwrap(),
        default_spsp_account: None,
        admin_auth_token: "admin".to_string(),
        redis_connection: connection_info2.clone(),
        btp_address: ([127, 0, 0, 1], get_open_port(None)).into(),
        http_address: ([127, 0, 0, 1], node2_http).into(),
        settlement_address: ([127, 0, 0, 1], node2_settlement).into(),
        secret_seed: node2_secret,
        route_broadcast_interval: Some(200),
    };
    runtime.spawn(
        start_eth_engine(connection_info2, node2_engine_address, bob_key, node2_settlement).and_then(
            move |_| {
                node2
                    .insert_account(AccountDetails {
                        ilp_address: Address::from_str("example.bob").unwrap(),
                        username: Username::from_str("bob").unwrap(),
                        asset_code: "ETH".to_string(),
                        asset_scale: eth_decimals,
                        btp_incoming_token: None,
                        btp_uri: None,
                        http_endpoint: None,
                        http_incoming_token: Some("in_bob".to_string()),
                        http_outgoing_token: None,
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
                    })
                    .and_then(move |_| {
                        node2
                            .insert_account(AccountDetails {
                                ilp_address: Address::from_str("example.alice").unwrap(),
                                username: Username::from_str("alice").unwrap(),
                                asset_code: "ETH".to_string(),
                                asset_scale: eth_decimals,
                                btp_incoming_token: None,
                                btp_uri: None,
                                http_endpoint: Some(format!("http://localhost:{}/ilp", node1_http)),
                                http_incoming_token: Some("bob".to_string()),
                                http_outgoing_token: Some("bob:alice".to_string()),
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
                            })
                            .and_then(move |_| node2.serve())
                    })
            },
        ),
    );

    runtime
        .block_on(
            // Wait for the nodes to spin up
            delay(1000)
                .map_err(|_| panic!("Something strange happened"))
                .and_then(move |_| {
                    // The 2 nodes are peered, we make a POST to the engine's
                    // create account endpoint so that they trade addresses.
                    // This would happen automatically if we inserted the
                    // accounts via the Accounts API.
                    let bob_addr = Address::from_str("example.bob").unwrap();
                    let bob_addr2 = bob_addr.clone();
                    let alice_addr = Address::from_str("example.alice").unwrap();
                    join_all(vec![
                        get_all_accounts(node1_http, "hi_alice").map(accounts_to_ids),
                        get_all_accounts(node2_http, "admin").map(accounts_to_ids),
                    ])
                    .and_then(move |ids| {
                        let node1_ids = ids[0].clone();
                        let node2_ids = ids[1].clone();

                        let bob = node1_ids.get(&bob_addr2).unwrap().to_owned();
                        let alice = node2_ids.get(&alice_addr).unwrap().to_owned();

                        let create1 = create_account_on_engine(node1_engine, bob);
                        let create2 = create_account_on_engine(node2_engine, alice);

                        // Make 4 subsequent payments (we could also do a 71 payment
                        // directly)
                        let send1 = send_money_to_username(
                            node1_http, node2_http, 10, "bob", "alice", "in_alice",
                        );
                        let send2 = send_money_to_username(
                            node1_http, node2_http, 20, "bob", "alice", "in_alice",
                        );
                        let send3 = send_money_to_username(
                            node1_http, node2_http, 39, "bob", "alice", "in_alice",
                        );
                        let send4 = send_money_to_username(
                            node1_http, node2_http, 1, "bob", "alice", "in_alice",
                        );

                        let get_balances = move || {
                            join_all(vec![
                                get_balance("bob", node1_http, "alice"),
                                get_balance("alice", node2_http, "bob"),
                            ])
                        };

                        create1
                            .and_then(move |_| create2)
                            .and_then(move |_| send1)
                            .and_then(move |_| get_balances())
                            .and_then(move |ret| {
                                assert_eq!(ret[0], 10);
                                assert_eq!(ret[1], -10);
                                Ok(())
                            })
                            .and_then(move |_| send2)
                            .and_then(move |_| get_balances())
                            .and_then(move |ret| {
                                assert_eq!(ret[0], 30);
                                assert_eq!(ret[1], -30);
                                Ok(())
                            })
                            .and_then(move |_| send3)
                            .and_then(move |_| get_balances())
                            .and_then(move |ret| {
                                assert_eq!(ret[0], 69);
                                assert_eq!(ret[1], -69);
                                Ok(())
                            })
                            // Up to here, Alice's balance should be -69 and Bob's
                            // balance should be 69. Once we make 1 more payment, we
                            // exceed the settle_threshold and thus a settlement is made
                            .and_then(move |_| send4)
                            .and_then(move |_| {
                                // Wait a few seconds so that the receiver's engine
                                // gets the data
                                delay(5000)
                                    .map_err(move |_| panic!("Weird error."))
                                    .and_then(move |_| {
                                        // Since the credit connection reached -70, and the
                                        // settle_to is -10, a 60 Wei transaction is made.
                                        get_balances().and_then(move |ret| {
                                            assert_eq!(ret[0], 10);
                                            assert_eq!(ret[1], -10);
                                            ganache_pid.kill().unwrap();
                                            Ok(())
                                        })
                                    })
                            })
                    })
                }),
        )
        .unwrap();
}
