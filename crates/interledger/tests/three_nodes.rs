#![recursion_limit = "128"]

use env_logger;
use futures::Future;
use interledger::{
    cli,
    node::{AccountDetails, InterledgerNode},
};
use interledger_packet::Address;
use interledger_service::Username;
use serde_json::json;
use std::str::FromStr;
use tokio::runtime::Builder as RuntimeBuilder;

mod redis_helpers;
use redis_helpers::*;

mod test_helpers;
use test_helpers::*;

#[test]
fn three_nodes() {
    // Nodes 1 and 2 are peers, Node 2 is the parent of Node 3
    let _ = env_logger::try_init();
    let context = TestContext::new();

    // Each node will use its own DB within the redis instance
    let mut connection_info1 = context.get_client_connection_info();
    connection_info1.db = 1;
    let mut connection_info2 = context.get_client_connection_info();
    connection_info2.db = 2;
    let mut connection_info3 = context.get_client_connection_info();
    connection_info3.db = 3;

    let node1_http = get_open_port(Some(3010));
    let node1_settlement = get_open_port(Some(3011));
    let node2_http = get_open_port(Some(3020));
    let node2_settlement = get_open_port(Some(3021));
    let node2_btp = get_open_port(Some(3022));
    let node3_http = get_open_port(Some(3030));
    let node3_settlement = get_open_port(Some(3031));

    let mut runtime = RuntimeBuilder::new()
        .panic_handler(|_| panic!("Tokio worker panicked"))
        .build()
        .unwrap();

    let node1 = InterledgerNode {
        ilp_address: Address::from_str("example.one").unwrap(),
        default_spsp_account: Some(Username::from_str("one").unwrap()),
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
        node1_clone
            .insert_account(AccountDetails {
                ilp_address: Address::from_str("example.one").unwrap(),
                username: Username::from_str("alice").unwrap(),
                asset_code: "XYZ".to_string(),
                asset_scale: 9,
                btp_incoming_token: None,
                btp_uri: None,
                http_endpoint: None,
                http_incoming_token: Some("default account holder".to_string()),
                http_outgoing_token: None,
                max_packet_amount: u64::max_value(),
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
            .and_then(move |_|
        // TODO insert the accounts via HTTP request
        node1_clone
            .insert_account(AccountDetails {
                ilp_address: Address::from_str("example.two").unwrap(),
                username: Username::from_str("bob").unwrap(),
                asset_code: "XYZ".to_string(),
                asset_scale: 9,
                btp_incoming_token: None,
                btp_uri: None,
                http_endpoint: Some(format!("http://localhost:{}/ilp", node2_http)),
                http_incoming_token: Some("two".to_string()), // usrename from other party
                http_outgoing_token: Some("alice:one".to_string()), // our username
                max_packet_amount: u64::max_value(),
                min_balance: Some(-1_000_000_000),
                settle_threshold: None,
                settle_to: None,
                send_routes: true,
                receive_routes: true,
                routing_relation: Some("Peer".to_string()),
                round_trip_time: None,
                packets_per_minute_limit: None,
                amount_per_minute_limit: None,
                settlement_engine_url: None,
            }))
            .and_then(move |_| node1.serve()),
    );

    let node2 = InterledgerNode {
        ilp_address: Address::from_str("example.two").unwrap(),
        default_spsp_account: Some(Username::from_str("two").unwrap()),
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
        node2_clone
            .insert_account(AccountDetails {
                ilp_address: Address::from_str("example.one").unwrap(),
                username: Username::from_str("alice").unwrap(),
                asset_code: "XYZ".to_string(),
                asset_scale: 9,
                btp_incoming_token: None,
                btp_uri: None,
                http_endpoint: Some(format!("http://localhost:{}/ilp", node1_http)),
                http_incoming_token: Some("one".to_string()),
                http_outgoing_token: Some("bob:two".to_string()),
                max_packet_amount: u64::max_value(),
                min_balance: None,
                settle_threshold: None,
                settle_to: None,
                send_routes: true,
                receive_routes: true,
                routing_relation: Some("Peer".to_string()),
                round_trip_time: None,
                packets_per_minute_limit: None,
                amount_per_minute_limit: None,
                settlement_engine_url: None,
            })
            .and_then(move |_| {
                node2_clone.insert_account(AccountDetails {
                    ilp_address: Address::from_str("example.two.three").unwrap(),
                    username: Username::from_str("charlie").unwrap(),
                    asset_code: "ABC".to_string(),
                    asset_scale: 6,
                    btp_incoming_token: Some("three".to_string()),
                    btp_uri: None,
                    http_endpoint: None,
                    http_incoming_token: Some("three".to_string()),
                    http_outgoing_token: None,
                    max_packet_amount: u64::max_value(),
                    min_balance: Some(-1_000_000_000),
                    settle_threshold: None,
                    settle_to: None,
                    send_routes: true,
                    receive_routes: false,
                    routing_relation: Some("Child".to_string()),
                    round_trip_time: None,
                    packets_per_minute_limit: None,
                    amount_per_minute_limit: None,
                    settlement_engine_url: None,
                })
            })
            .and_then(move |_| node2.serve())
            .and_then(move |_| {
                let client = reqwest::r#async::Client::new();
                client
                    .put(&format!("http://localhost:{}/rates", node2_http))
                    .header("Authorization", "Bearer admin")
                    .json(&json!({"ABC": 2, "XYZ": 1}))
                    .send()
                    .map_err(|err| panic!(err))
                    .and_then(|res| {
                        res.error_for_status()
                            .expect("Error setting exchange rates");
                        Ok(())
                    })
            }),
    );

    let node3 = InterledgerNode {
        ilp_address: Address::from_str("example.two.three").unwrap(),
        default_spsp_account: Some(Username::from_str("three").unwrap()),
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
            node3_clone
                .insert_account(AccountDetails {
                    ilp_address: Address::from_str("example.two.three").unwrap(),
                    username: Username::from_str("charlie").unwrap(),
                    asset_code: "ABC".to_string(),
                    asset_scale: 6,
                    btp_incoming_token: None,
                    btp_uri: None,
                    http_endpoint: None,
                    http_incoming_token: Some("default account holder".to_string()),
                    http_outgoing_token: None,
                    max_packet_amount: u64::max_value(),
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
                    node3_clone.insert_account(AccountDetails {
                        ilp_address: Address::from_str("example.two").unwrap(),
                        username: Username::from_str("bob").unwrap(),
                        asset_code: "ABC".to_string(),
                        asset_scale: 6,
                        btp_incoming_token: None,
                        btp_uri: Some(format!("btp+ws://charlie:three@localhost:{}", node2_btp)),
                        http_endpoint: None,
                        http_incoming_token: None,
                        http_outgoing_token: None,
                        max_packet_amount: u64::max_value(),
                        min_balance: Some(-1_000_000_000),
                        settle_threshold: None,
                        settle_to: None,
                        send_routes: false,
                        receive_routes: true,
                        routing_relation: Some("Parent".to_string()),
                        round_trip_time: None,
                        packets_per_minute_limit: None,
                        amount_per_minute_limit: None,
                        settlement_engine_url: None,
                    })
                })
                .and_then(move |_| node3.serve())
        }),
    );

    runtime
        .block_on(
            // Wait for the nodes to spin up
            delay(500)
                .map_err(|_| panic!("Something strange happened"))
                .and_then(move |_| {
                    let send_1_to_3 = send_money_to_username(
                        node1_http,
                        node3_http,
                        1000,
                        "charlie",
                        "alice",
                        "default account holder",
                    );
                    let send_3_to_1 = send_money_to_username(
                        node3_http,
                        node1_http,
                        1000,
                        "alice",
                        "charlie",
                        "default account holder",
                    );

                    let get_balances = move || {
                        futures::future::join_all(vec![
                            get_balance("alice", node1_http, "admin"),
                            get_balance("charlie", node2_http, "admin"),
                            get_balance("charlie", node3_http, "admin"),
                        ])
                    };

                    // // Node 1 sends 1000 to Node 3. However, Node1's scale is 9,
                    // // while Node 3's scale is 6. This means that Node 3 will
                    // // see 1000x less. In addition, the conversion rate is 2:1
                    // // for 3's asset, so he will receive 2 total.
                    send_1_to_3
                        .map_err(|err| {
                            eprintln!("Error sending from node 1 to node 3: {:?}", err);
                            err
                        })
                        .and_then(move |_| {
                            get_balances().and_then(move |ret| {
                                assert_eq!(ret[0], -1000);
                                assert_eq!(ret[1], 2);
                                assert_eq!(ret[2], 2);
                                Ok(())
                            })
                        })
                        .and_then(move |_| {
                            send_3_to_1.map_err(|err| {
                                eprintln!("Error sending from node 3 to node 1: {:?}", err);
                                err
                            })
                        })
                        .and_then(move |_| {
                            get_balances().and_then(move |ret| {
                                assert_eq!(ret[0], 499_000);
                                assert_eq!(ret[1], -998);
                                assert_eq!(ret[2], -998);
                                Ok(())
                            })
                        })
                }),
        )
        .unwrap();
}
