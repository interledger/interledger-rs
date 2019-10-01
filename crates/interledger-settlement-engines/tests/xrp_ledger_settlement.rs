#![recursion_limit = "128"]

use futures::future::join_all;
use futures::Future;
use ilp_node::{random_secret, InterledgerNode};
use interledger::{api::AccountDetails, packet::Address, service::Username};
use secrecy::SecretString;
use std::str::FromStr;
use tokio::runtime::Builder as RuntimeBuilder;

mod test_helpers;
use test_helpers::{
    accounts_to_ids, create_account_on_engine, get_all_accounts, get_balance, redis_helpers::*,
    send_money_to_username, start_xrp_engine,
};

#[test]
/// In this test we have Alice and Bob who have peered with each other and run
/// XRP ledger settlement engines. Alice proceeds to make SPSP payments to
/// Bob, until she eventually reaches Bob's `settle_threshold`. Once that's
/// exceeded, her engine makes a settlement request to Bob. Alice's connector
/// immediately applies the balance change. Bob's engine listens for incoming
/// transactions, and once the transaction has sufficient confirmations it
/// lets Bob's connector know about it, so that it adjusts their credit.
fn xrp_ledger_settlement() {
    let xrp_decimals = 6;
    // Nodes 1 and 2 are peers, Node 2 is the parent of Node 3
    let _ = env_logger::try_init();
    let context = TestContext::new();

    // Each node will use its own DB within the redis instance
    let mut connection_info1 = context.get_client_connection_info();
    connection_info1.db = 1;
    let mut connection_info2 = context.get_client_connection_info();
    connection_info2.db = 2;

    let node1_http = get_open_port(Some(3010));
    let node1_settlement = get_open_port(Some(3011));
    let node1_engine = get_open_port(Some(3012));

    let node2_http = get_open_port(Some(3020));
    let node2_settlement = get_open_port(Some(3021));
    let node2_engine = get_open_port(Some(3022));

    // spawn 2 redis servers for the XRP engines
    let alice_redis_port = get_open_port(Some(6379));
    let bob_redis_port = get_open_port(Some(6380));
    let mut alice_engine_redis = RedisServer::spawn_with_port(alice_redis_port);
    let mut bob_engine_redis = RedisServer::spawn_with_port(bob_redis_port);
    let mut engine_alice =
        start_xrp_engine("http://localhost:3011", alice_redis_port, node1_engine);
    let mut engine_bob = start_xrp_engine("http://localhost:3021", bob_redis_port, node2_engine);
    std::thread::sleep(std::time::Duration::from_secs(10));

    let mut runtime = RuntimeBuilder::new()
        .panic_handler(|_| panic!("Tokio worker panicked"))
        .build()
        .unwrap();

    let node1_secret = random_secret();
    let node1 = InterledgerNode {
        ilp_address: Some(Address::from_str("example.alice").unwrap()),
        default_spsp_account: None,
        admin_auth_token: "hi_alice".to_string(),
        redis_connection: connection_info1.clone(),
        http_bind_address: ([127, 0, 0, 1], node1_http).into(),
        settlement_api_bind_address: ([127, 0, 0, 1], node1_settlement).into(),
        secret_seed: node1_secret,
        route_broadcast_interval: Some(200),
        exchange_rate_poll_interval: 60000,
        exchange_rate_provider: None,
        exchange_rate_spread: 0.0,
    };
    let node1_clone = node1.clone();
    runtime.spawn(
        // TODO insert the accounts via HTTP request
        node1_clone
            .insert_account(AccountDetails {
                ilp_address: Some(Address::from_str("example.alice").unwrap()),
                username: Username::from_str("alice").unwrap(),
                asset_code: "XRP".to_string(),
                asset_scale: xrp_decimals,
                ilp_over_btp_url: None,
                ilp_over_btp_incoming_token: None,
                ilp_over_btp_outgoing_token: None,
                ilp_over_http_url: None,
                ilp_over_http_incoming_token: Some(SecretString::new("in_alice".to_string())),
                ilp_over_http_outgoing_token: None,
                max_packet_amount: 10,
                min_balance: None,
                settle_threshold: None,
                settle_to: Some(-10),
                routing_relation: None,
                round_trip_time: None,
                packets_per_minute_limit: None,
                amount_per_minute_limit: None,
                settlement_engine_url: None,
            })
            .and_then(move |_| {
                node1_clone.insert_account(AccountDetails {
                    ilp_address: Some(Address::from_str("example.bob").unwrap()),
                    username: Username::from_str("bob").unwrap(),
                    asset_code: "XRP".to_string(),
                    asset_scale: xrp_decimals,
                    ilp_over_btp_url: None,
                    ilp_over_btp_incoming_token: None,
                    ilp_over_btp_outgoing_token: None,
                    ilp_over_http_url: Some(format!("http://localhost:{}/ilp", node2_http)),
                    ilp_over_http_incoming_token: Some(SecretString::new("alice".to_string())),
                    ilp_over_http_outgoing_token: Some(SecretString::new("alice:bob".to_string())),
                    max_packet_amount: 10,
                    min_balance: Some(-100),
                    settle_threshold: Some(70),
                    settle_to: Some(10),
                    routing_relation: None,
                    round_trip_time: None,
                    packets_per_minute_limit: None,
                    amount_per_minute_limit: None,
                    settlement_engine_url: Some(format!("http://localhost:{}", node1_engine)),
                })
            })
            .and_then(move |_| node1.serve()),
    );

    let node2_secret = random_secret();
    let node2 = InterledgerNode {
        ilp_address: Some(Address::from_str("example.bob").unwrap()),
        default_spsp_account: None,
        admin_auth_token: "admin".to_string(),
        redis_connection: connection_info2.clone(),
        http_bind_address: ([127, 0, 0, 1], node2_http).into(),
        settlement_api_bind_address: ([127, 0, 0, 1], node2_settlement).into(),
        secret_seed: node2_secret,
        route_broadcast_interval: Some(200),
        exchange_rate_poll_interval: 60000,
        exchange_rate_provider: None,
        exchange_rate_spread: 0.0,
    };
    runtime.spawn(
        node2
            .insert_account(AccountDetails {
                ilp_address: Some(Address::from_str("example.bob").unwrap()),
                username: Username::from_str("bob").unwrap(),
                asset_code: "XRP".to_string(),
                asset_scale: xrp_decimals,
                ilp_over_btp_url: None,
                ilp_over_btp_incoming_token: None,
                ilp_over_btp_outgoing_token: None,
                ilp_over_http_url: None,
                ilp_over_http_incoming_token: Some(SecretString::new("in_bob".to_string())),
                ilp_over_http_outgoing_token: None,
                max_packet_amount: 10,
                min_balance: None,
                settle_threshold: None,
                settle_to: None,
                routing_relation: None,
                round_trip_time: None,
                packets_per_minute_limit: None,
                amount_per_minute_limit: None,
                settlement_engine_url: None,
            })
            .and_then(move |_| {
                node2
                    .insert_account(AccountDetails {
                        ilp_address: Some(Address::from_str("example.alice").unwrap()),
                        username: Username::from_str("alice").unwrap(),
                        asset_code: "XRP".to_string(),
                        asset_scale: xrp_decimals,
                        ilp_over_btp_url: None,
                        ilp_over_btp_incoming_token: None,
                        ilp_over_btp_outgoing_token: None,
                        ilp_over_http_url: Some(format!("http://localhost:{}/ilp", node1_http)),
                        ilp_over_http_incoming_token: Some(SecretString::new("bob".to_string())),
                        ilp_over_http_outgoing_token: Some(SecretString::new(
                            "bob:alice".to_string(),
                        )),
                        max_packet_amount: 10,
                        min_balance: Some(-100),
                        settle_threshold: Some(70),
                        settle_to: Some(-10),
                        routing_relation: None,
                        round_trip_time: None,
                        packets_per_minute_limit: None,
                        amount_per_minute_limit: None,
                        settlement_engine_url: Some(format!("http://localhost:{}", node2_engine)),
                    })
                    .and_then(move |_| node2.serve())
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
                    let bob_addr = Address::from_str("example.bob").unwrap();
                    let bob_addr2 = bob_addr.clone();
                    let alice_addr = Address::from_str("example.alice").unwrap();
                    futures::future::join_all(vec![
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
                                // gets the data and applies it (longer than the
                                // Ethereum engine since we're using a public
                                // testnet here)
                                delay(10000)
                                    .map_err(move |_| panic!("Weird error."))
                                    .and_then(move |_| {
                                        // Since the credit connection reached -70, and the
                                        // settle_to is -10, a 60 Wei transaction is made.
                                        get_balances().and_then(move |ret| {
                                            assert_eq!(ret[0], 10);
                                            assert_eq!(ret[1], -10);
                                            engine_alice.kill().unwrap();
                                            alice_engine_redis.kill().unwrap();
                                            engine_bob.kill().unwrap();
                                            bob_engine_redis.kill().unwrap();
                                            Ok(())
                                        })
                                    })
                            })
                    })
                }),
        )
        .unwrap();
}
