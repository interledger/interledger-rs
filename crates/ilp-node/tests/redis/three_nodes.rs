use crate::redis_helpers::*;
use crate::test_helpers::*;
use ilp_node::InterledgerNode;
use interledger::packet::Address;
use serde_json::json;
use std::str::FromStr;

#[tokio::test]
async fn three_nodes() {
    // Nodes 1 and 2 are peers, Node 2 is the parent of Node 3
    let context = TestContext::new();

    // Each node will use its own DB within the redis instance
    let mut connection_info1 = context.get_client_connection_info();
    connection_info1.redis.db = 1;
    let mut connection_info2 = context.get_client_connection_info();
    connection_info2.redis.db = 2;
    let mut connection_info3 = context.get_client_connection_info();
    connection_info3.redis.db = 3;

    let node1_http = get_open_port(None);
    let node1_settlement = get_open_port(None);
    let node2_http = get_open_port(None);
    let node2_settlement = get_open_port(None);
    let node3_http = get_open_port(None);
    let node3_settlement = get_open_port(None);
    let alice_on_alice = json!({
        "ilp_address": "example.alice",
        "username": "alice_on_a",
        "asset_code": "XYZ",
        "asset_scale": 9,
        "ilp_over_http_incoming_token" : "default account holder",
    });
    let bob_on_alice = json!({
        "ilp_address": "example.bob",
        "username": "bob_on_a",
        "asset_code": "XYZ",
        "asset_scale": 9,
        "ilp_over_http_url": format!("http://localhost:{}/accounts/{}/ilp", node2_http, "alice_on_b"),
        "ilp_over_http_incoming_token" : "two",
        "ilp_over_http_outgoing_token" : "one",
        "min_balance": -1_000_000_000,
        "routing_relation": "Peer",
    });

    let alice_on_bob = json!({
        "ilp_address": "example.alice",
        "username": "alice_on_b",
        "asset_code": "XYZ",
        "asset_scale": 9,
        "ilp_over_http_url": format!("http://localhost:{}/accounts/{}/ilp", node1_http, "bob_on_a"),
        "ilp_over_http_incoming_token" : "one",
        "ilp_over_http_outgoing_token" : "two",
        "routing_relation": "Peer",
    });
    let charlie_on_bob = json!({
        "username": "charlie_on_b",
        "asset_code": "ABC",
        "asset_scale": 6,
        "ilp_over_btp_incoming_token" : "three",
        "min_balance": -1_000_000_000,
        "routing_relation": "Child",
    });

    let charlie_on_charlie = json!({
        "username": "charlie_on_c",
        "asset_code": "ABC",
        "asset_scale": 6,
        "ilp_over_http_incoming_token" : "default account holder",
    });
    let bob_on_charlie = json!({
        "ilp_address": "example.bob",
        "username": "bob_on_c",
        "asset_code": "ABC",
        "asset_scale": 6,
        "ilp_over_http_incoming_token" : "two",
        "ilp_over_http_outgoing_token": "three",
        "ilp_over_btp_url": format!("btp+ws://localhost:{}/accounts/{}/ilp/btp", node2_http, "charlie_on_b"),
        "ilp_over_btp_outgoing_token": "three",
        "min_balance": -1_000_000_000,
        "routing_relation": "Parent",
    });

    let node1: InterledgerNode = serde_json::from_value(json!({
        "ilp_address": "example.alice",
        "default_spsp_account": "alice_on_a",
        "admin_auth_token": "admin",
        "database_url": connection_info_to_string(connection_info1),
        "http_bind_address": format!("127.0.0.1:{}", node1_http),
        "settlement_api_bind_address": format!("127.0.0.1:{}", node1_settlement),
        "secret_seed": random_secret(),
        "route_broadcast_interval": Some(200),
        "exchange_rate": {
            "poll_interval": 60000,
        },
    }))
    .expect("Error creating node1.");

    let node2: InterledgerNode = serde_json::from_value(json!({
        "ilp_address": "example.bob",
        "admin_auth_token": "admin",
        "database_url": connection_info_to_string(connection_info2),
        "http_bind_address": format!("127.0.0.1:{}", node2_http),
        "settlement_api_bind_address": format!("127.0.0.1:{}", node2_settlement),
        "secret_seed": random_secret(),
        "route_broadcast_interval": Some(200),
        "exchange_rate": {
            "poll_interval": 60000,
            "spread": 0.02, // take a 2% spread
        },
    }))
    .expect("Error creating node2.");

    let node3: InterledgerNode = serde_json::from_value(json!({
        "default_spsp_account": "charlie_on_c",
        "admin_auth_token": "admin",
        "database_url": connection_info_to_string(connection_info3),
        "http_bind_address": format!("127.0.0.1:{}", node3_http),
        "settlement_api_bind_address": format!("127.0.0.1:{}", node3_settlement),
        "secret_seed": random_secret(),
        "route_broadcast_interval": Some(200),
        "exchange_rate": {
            "poll_interval": 60000
        },
    }))
    .expect("Error creating node3.");

    node1.serve(None).await.unwrap();
    create_account_on_node(node1_http, alice_on_alice, "admin")
        .await
        .unwrap();
    create_account_on_node(node1_http, bob_on_alice, "admin")
        .await
        .unwrap();
    let client = reqwest::Client::new();
    client
        .put(&format!("http://localhost:{}/rates", node1_http))
        .header("Authorization", "Bearer admin")
        .json(&json!({"ABC": 1, "XYZ": 2.01}))
        .send()
        .await
        .unwrap();

    node2.serve(None).await.unwrap();
    create_account_on_node(node2_http, alice_on_bob, "admin")
        .await
        .unwrap();
    create_account_on_node(node2_http, charlie_on_bob, "admin")
        .await
        .unwrap();
    client
        .put(&format!("http://localhost:{}/rates", node2_http))
        .header("Authorization", "Bearer admin")
        .json(&json!({"ABC": 1, "XYZ": 2}))
        .send()
        .await
        .unwrap();

    node3.serve(None).await.unwrap();
    create_account_on_node(node3_http, charlie_on_charlie, "admin")
        .await
        .unwrap();
    create_account_on_node(node3_http, bob_on_charlie, "admin")
        .await
        .unwrap();
    client
        .put(&format!("http://localhost:{}/rates", node3_http))
        .header("Authorization", "Bearer admin")
        .json(&json!({"ABC": 1, "XYZ": 2}))
        .send()
        .await
        .unwrap();

    delay(1000).await;

    let get_balances = move || {
        futures::future::join_all(vec![
            get_balance("alice_on_a", node1_http, "admin"),
            get_balance("charlie_on_b", node2_http, "admin"),
            get_balance("charlie_on_c", node3_http, "admin"),
        ])
    };

    // Node 1 sends 1,000,000 to Node 3. However, Node1's scale is 9,
    // while Node 3's scale is 6. This means that Node 3 will
    // see 1000x less. Node 2's rate is 2:1, minus a 2% spread.
    // Node 1 has however set the rate to 2.01:1. The payment suceeds because
    // the end delivered result to Node 3 is 1960 which is slightly
    // over 0.975 * 2010, which is within the 2.5% slippage set by the sender.
    let receipt = send_money_to_username(
        node1_http,
        node3_http,
        1_000_000,
        "charlie_on_c",
        "alice_on_a",
        "default account holder",
    )
    .await
    .unwrap();

    assert_eq!(
        receipt.from,
        Address::from_str("example.alice").unwrap(),
        "Payment receipt incorrect (1)"
    );
    assert!(receipt
        .to
        .to_string()
        .starts_with("example.bob.charlie_on_b.charlie_on_c."));
    assert_eq!(receipt.source_asset_code, "XYZ");
    assert_eq!(receipt.source_asset_scale, 9);
    assert_eq!(receipt.source_amount, 1_000_000);
    assert_eq!(receipt.sent_amount, 1_000_000);
    assert_eq!(receipt.in_flight_amount, 0);
    assert_eq!(receipt.delivered_amount, 1960);
    assert_eq!(receipt.destination_asset_code.unwrap(), "ABC");
    assert_eq!(receipt.destination_asset_scale.unwrap(), 6);
    let ret = get_balances().await;
    let ret: Vec<_> = ret.into_iter().map(|r| r.unwrap()).collect();
    // -1000000 divided by asset scale 9
    assert_eq!(
        ret[0],
        BalanceData {
            asset_code: "XYZ".to_owned(),
            balance: -1_000_000.0 / 1e9
        }
    );
    // 1960 divided by asset scale 6
    assert_eq!(
        ret[1],
        BalanceData {
            asset_code: "ABC".to_owned(),
            balance: 1960.0 / 1e6
        }
    );
    // 1960 divided by asset scale 6
    assert_eq!(
        ret[2],
        BalanceData {
            asset_code: "ABC".to_owned(),
            balance: 1960.0 / 1e6
        }
    );

    // Node 3 sends 1,000 to Node 1. However, Node 1's scale is 9,
    // while Node 3's scale is 6. This means that Node 1 will
    // see 1000x more. Node 2's rate is 1:2, minus a 2% spread.
    // Node 3 should receive 495,500 units total.
    let receipt = send_money_to_username(
        node3_http,
        node1_http,
        1000,
        "alice_on_a",
        "charlie_on_c",
        "default account holder",
    )
    .await
    .unwrap();

    assert_eq!(
        receipt.from,
        Address::from_str("example.bob.charlie_on_b.charlie_on_c").unwrap(),
        "Payment receipt incorrect (2)"
    );
    assert!(receipt.to.to_string().starts_with("example.alice"));
    assert_eq!(receipt.source_asset_code, "ABC");
    assert_eq!(receipt.source_asset_scale, 6);
    assert_eq!(receipt.source_amount, 1000);
    assert_eq!(receipt.sent_amount, 1000);
    assert_eq!(receipt.in_flight_amount, 0);
    assert_eq!(receipt.delivered_amount, 490_000);
    assert_eq!(receipt.destination_asset_code.unwrap(), "XYZ");
    assert_eq!(receipt.destination_asset_scale.unwrap(), 9);
    let ret = get_balances().await;
    let ret: Vec<_> = ret.into_iter().map(|r| r.unwrap()).collect();
    // (490,000 - 1,000,000) divided by asset scale 9
    assert_eq!(
        ret[0],
        BalanceData {
            asset_code: "XYZ".to_owned(),
            balance: (490_000.0 - 1_000_000.0) / 1e9
        }
    );
    // (1,960 - 1,000) divided by asset scale 6
    assert_eq!(
        ret[1],
        BalanceData {
            asset_code: "ABC".to_owned(),
            balance: (1960.0 - 1000.0) / 1e6
        }
    );
    // (1,960 - 1,000) divided by asset scale 6
    assert_eq!(
        ret[2],
        BalanceData {
            asset_code: "ABC".to_owned(),
            balance: (1960.0 - 1000.0) / 1e6
        }
    );
}
