use crate::redis_helpers::*;
use crate::test_helpers::*;
use ilp_node::InterledgerNode;
use serde_json::{self, json};
// use tracing::error_span;
// use tracing_futures::Instrument;

#[tokio::test]
async fn two_nodes_btp() {
    // Nodes 1 and 2 are peers, Node 2 is the parent of Node 2
    let context = TestContext::new();

    // Each node will use its own DB within the redis instance
    let mut connection_info1 = context.get_client_connection_info();
    connection_info1.redis.db = 1;
    let mut connection_info2 = context.get_client_connection_info();
    connection_info2.redis.db = 2;

    let node_a_http = get_open_port(None);
    let node_a_settlement = get_open_port(None);
    let node_b_http = get_open_port(None);
    let node_b_settlement = get_open_port(None);

    let alice_on_a = json!({
        "username": "alice_on_a",
        "asset_code": "XYZ",
        "asset_scale": 9,
        "ilp_over_http_incoming_token" : "default account holder",
    });
    let b_on_a = json!({
        "username": "b_on_a",
        "asset_code": "XYZ",
        "asset_scale": 9,
        "ilp_over_btp_url": format!("btp+ws://localhost:{}/accounts/{}/ilp/btp", node_b_http, "a_on_b"),
        "ilp_over_btp_outgoing_token" : "token",
        "routing_relation": "Parent",
    });

    let a_on_b = json!({
        "username": "a_on_b",
        "asset_code": "XYZ",
        "asset_scale": 9,
        // TODO make sure this field can exactly match the outgoing token on the other side
        "ilp_over_btp_incoming_token" : "token",
        "routing_relation": "Child",
    });
    let bob_on_b = json!({
        "username": "bob_on_b",
        "asset_code": "XYZ",
        "asset_scale": 9,
        "ilp_over_http_incoming_token" : "default account holder",
    });

    let node_a: InterledgerNode = serde_json::from_value(json!({
        "admin_auth_token": "admin",
        "database_url": connection_info_to_string(connection_info1),
        "http_bind_address": format!("127.0.0.1:{}", node_a_http),
        "settlement_api_bind_address": format!("127.0.0.1:{}", node_a_settlement),
        "secret_seed": random_secret(),
        "route_broadcast_interval": 200,
        "exchange_rate": {
            "poll_interval": 60000
        },
    }))
    .expect("Error creating node_a.");

    let node_b: InterledgerNode = serde_json::from_value(json!({
        "ilp_address": "example.parent",
        "default_spsp_account": "bob_on_b",
        "admin_auth_token": "admin",
        "database_url": connection_info_to_string(connection_info2),
        "http_bind_address": format!("127.0.0.1:{}", node_b_http),
        "settlement_api_bind_address": format!("127.0.0.1:{}", node_b_settlement),
        "secret_seed": random_secret(),
        "route_broadcast_interval": Some(200),
        "exchange_rate": {
            "poll_interval": 60000
        },
    }))
    .expect("Error creating node_b.");

    node_b.serve(None).await.unwrap();
    create_account_on_node(node_b_http, a_on_b, "admin")
        .await
        .unwrap();
    create_account_on_node(node_b_http, bob_on_b, "admin")
        .await
        .unwrap();

    node_a.serve(None).await.unwrap();
    create_account_on_node(node_a_http, alice_on_a, "admin")
        .await
        .unwrap();
    create_account_on_node(node_a_http, b_on_a, "admin")
        .await
        .unwrap();

    let get_balances = move || {
        futures::future::join_all(vec![
            get_balance("alice_on_a", node_a_http, "admin"),
            get_balance("bob_on_b", node_b_http, "admin"),
        ])
    };

    send_money_to_username(
        node_a_http,
        node_b_http,
        1000,
        "bob_on_b",
        "alice_on_a",
        "default account holder",
    )
    .await
    .unwrap();

    let ret = get_balances().await;
    let ret: Vec<_> = ret.into_iter().map(|r| r.unwrap()).collect();
    assert_eq!(
        ret[0],
        BalanceData {
            asset_code: "XYZ".to_owned(),
            balance: -1e-6
        }
    );
    assert_eq!(
        ret[1],
        BalanceData {
            asset_code: "XYZ".to_owned(),
            balance: 1e-6
        }
    );

    send_money_to_username(
        node_b_http,
        node_a_http,
        2000,
        "alice_on_a",
        "bob_on_b",
        "default account holder",
    )
    .await
    .unwrap();

    let ret = get_balances().await;
    let ret: Vec<_> = ret.into_iter().map(|r| r.unwrap()).collect();
    assert_eq!(
        ret[0],
        BalanceData {
            asset_code: "XYZ".to_owned(),
            balance: 1e-6
        }
    );
    assert_eq!(
        ret[1],
        BalanceData {
            asset_code: "XYZ".to_owned(),
            balance: -1e-6
        }
    );
}
