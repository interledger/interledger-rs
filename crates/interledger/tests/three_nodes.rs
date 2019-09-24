#![recursion_limit = "128"]

use futures::{future::join_all, Future};
use interledger::node::{random_secret, InterledgerNode};
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

    let alice_on_alice = json!({
        "ilp_address": "example.alice",
        "username": "alice",
        "asset_code": "XYZ",
        "asset_scale": 9,
        "http_incoming_token" : "default account holder",
    });
    let bob_on_alice = json!({
        "ilp_address": "example.bob",
        "username": "bob",
        "asset_code": "XYZ",
        "asset_scale": 9,
        "http_endpoint": format!("http://localhost:{}/ilp", node2_http),
        "http_incoming_token" : "two",
        "http_outgoing_token" : "alice:one",
        "min_balance": -1_000_000_000,
        "routing_relation": "Peer",
    });

    let alice_on_bob = json!({
        "ilp_address": "example.alice",
        "username": "alice",
        "asset_code": "XYZ",
        "asset_scale": 9,
        "http_endpoint": format!("http://localhost:{}/ilp", node1_http),
        "http_incoming_token" : "one",
        "http_outgoing_token" : "bob:two",
        "routing_relation": "Peer",
    });
    let charlie_on_bob = json!({
        "username": "charlie",
        "asset_code": "ABC",
        "asset_scale": 6,
        "btp_incoming_token" : "three",
        "http_incoming_token" : "three",
        "min_balance": -1_000_000_000,
        "routing_relation": "Child",
    });

    let charlie_on_charlie = json!({
        "username": "charlie",
        "asset_code": "ABC",
        "asset_scale": 6,
        "http_incoming_token" : "default account holder",
    });
    let bob_on_charlie = json!({
        "ilp_address": "example.bob",
        "username": "bob",
        "asset_code": "ABC",
        "asset_scale": 6,
        "http_incoming_token" : "two",
        "http_outgoing_token": "charlie:three",
        "http_endpoint": format!("http://localhost:{}/ilp", node2_http),
        "btp_uri": format!("btp+ws://charlie:three@localhost:{}", node2_btp),
        "min_balance": -1_000_000_000,
        "routing_relation": "Parent",
    });

    let node1 = InterledgerNode {
        ilp_address: Address::from_str("example.alice").unwrap(),
        default_spsp_account: Some(Username::from_str("alice").unwrap()),
        admin_auth_token: "admin".to_string(),
        redis_connection: connection_info1,
        btp_bind_address: ([127, 0, 0, 1], get_open_port(None)).into(),
        http_bind_address: ([127, 0, 0, 1], node1_http).into(),
        settlement_api_bind_address: ([127, 0, 0, 1], node1_settlement).into(),
        secret_seed: random_secret(),
        route_broadcast_interval: Some(200),
        exchange_rate_poll_interval: 60000,
        exchange_rate_provider: None,
        exchange_rate_spread: 0.0,
    };

    let node2 = InterledgerNode {
        ilp_address: Address::from_str("example.bob").unwrap(),
        default_spsp_account: Some(Username::from_str("bob").unwrap()),
        admin_auth_token: "admin".to_string(),
        redis_connection: connection_info2,
        btp_bind_address: ([127, 0, 0, 1], node2_btp).into(),
        http_bind_address: ([127, 0, 0, 1], node2_http).into(),
        settlement_api_bind_address: ([127, 0, 0, 1], node2_settlement).into(),
        secret_seed: random_secret(),
        route_broadcast_interval: Some(200),
        exchange_rate_poll_interval: 60000,
        exchange_rate_provider: None,
        exchange_rate_spread: 0.0,
    };

    let node3 = InterledgerNode {
        ilp_address: Address::from_str("local.host").unwrap(), // We should set this to local.host. Adding a parent should update our address by making an ILDCP request, followed by updating our routing table by making a RouteControlRequest to which the parent responds with a RouteUpdateRequest
        default_spsp_account: Some(Username::from_str("charlie").unwrap()),
        admin_auth_token: "admin".to_string(),
        redis_connection: connection_info3,
        btp_bind_address: ([127, 0, 0, 1], get_open_port(None)).into(),
        http_bind_address: ([127, 0, 0, 1], node3_http).into(),
        settlement_api_bind_address: ([127, 0, 0, 1], node3_settlement).into(),
        secret_seed: random_secret(),
        route_broadcast_interval: Some(200),
        exchange_rate_poll_interval: 60000,
        exchange_rate_provider: None,
        exchange_rate_spread: 0.0,
    };

    let alice_fut = join_all(vec![
        create_account_on_node(node1_http, alice_on_alice, "admin"),
        create_account_on_node(node1_http, bob_on_alice, "admin"),
    ]);

    runtime.spawn(
        node1
            .serve()
            .and_then(move |_| alice_fut)
            .and_then(move |_| Ok(())),
    );

    let bob_fut = join_all(vec![
        create_account_on_node(node2_http, alice_on_bob, "admin"),
        create_account_on_node(node2_http, charlie_on_bob, "admin"),
    ]);

    runtime.spawn(node2.serve().and_then(move |_| bob_fut).and_then(move |_| {
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
    }));

    let charlie_fut = create_account_on_node(node3_http, bob_on_charlie, "admin")
        .and_then(move |_| create_account_on_node(node3_http, charlie_on_charlie, "admin"));

    runtime.spawn(
        node3
            .serve()
            .and_then(move |_| charlie_fut)
            .and_then(move |_| Ok(())),
    );

    runtime
        .block_on(
            // Wait for the nodes to spin up
            delay(1000)
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
