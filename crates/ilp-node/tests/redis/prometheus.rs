use crate::redis_helpers::*;
use crate::test_helpers::*;
use futures::TryFutureExt;
use ilp_node::InterledgerNode;
use reqwest::Client;
use serde_json::{self, json};

#[tokio::test]
async fn prometheus() {
    // Nodes 1 and 2 are peers, Node 2 is the parent of Node 2
    tracing_subscriber::fmt::Subscriber::builder()
        .with_timer(tracing_subscriber::fmt::time::ChronoUtc::rfc3339())
        .with_env_filter(tracing_subscriber::filter::EnvFilter::from_default_env())
        .try_init()
        .unwrap_or(());
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
    let prometheus_port = get_open_port(None);

    let alice_on_a = json!({
        "ilp_address": "example.node_a.alice",
        "username": "alice_on_a",
        "asset_code": "XYZ",
        "asset_scale": 9,
        "ilp_over_http_incoming_token" : "token",
    });
    let b_on_a = json!({
        "ilp_address": "example.node_b",
        "username": "node_b",
        "asset_code": "XYZ",
        "asset_scale": 9,
        "ilp_over_http_url": format!("http://localhost:{}/accounts/{}/ilp", node_b_http, "node_a"),
        "ilp_over_http_incoming_token" : "token",
        "ilp_over_http_outgoing_token" : "token",
        "routing_relation": "Peer",
    });

    let a_on_b = json!({
        "ilp_address": "example.node_a",
        "username": "node_a",
        "asset_code": "XYZ",
        "asset_scale": 9,
        "ilp_over_http_url": format!("http://localhost:{}/accounts/{}/ilp", node_a_http, "node_b"),
        "ilp_over_http_incoming_token" : "token",
        "ilp_over_http_outgoing_token" : "token",
        "routing_relation": "Peer",
    });

    let bob_on_b = json!({
        "ilp_address": "example.node_b.bob",
        "username": "bob_on_b",
        "asset_code": "XYZ",
        "asset_scale": 6,
        "ilp_over_http_incoming_token" : "token",
    });

    let node_a: InterledgerNode = serde_json::from_value(json!({
        "admin_auth_token": "admin",
        "database_url": connection_info_to_string(connection_info1),
        "http_bind_address": format!("127.0.0.1:{}", node_a_http),
        "settlement_api_bind_address": format!("127.0.0.1:{}", node_a_settlement),
        "secret_seed": random_secret(),
        "prometheus": {
            "bind_address": format!("127.0.0.1:{}", prometheus_port),
            "histogram_window": 10000,
            "histogram_granularity": 1000,
        }
    }))
    .unwrap();

    let node_b: InterledgerNode = serde_json::from_value(json!({
        "ilp_address": "example.parent",
        "default_spsp_account": "bob_on_b",
        "admin_auth_token": "admin",
        "database_url": connection_info_to_string(connection_info2),
        "http_bind_address": format!("127.0.0.1:{}", node_b_http),
        "settlement_api_bind_address": format!("127.0.0.1:{}", node_b_settlement),
        "secret_seed": random_secret(),
    }))
    .unwrap();

    node_a.serve(None).await.unwrap();
    node_b.serve(None).await.unwrap();

    create_account_on_node(node_b_http, a_on_b, "admin")
        .await
        .unwrap();
    create_account_on_node(node_b_http, bob_on_b, "admin")
        .await
        .unwrap();
    create_account_on_node(node_a_http, alice_on_a, "admin")
        .await
        .unwrap();
    create_account_on_node(node_a_http, b_on_a, "admin")
        .await
        .unwrap();

    let check_metrics = move || {
        Client::new()
            .get(&format!("http://127.0.0.1:{}", prometheus_port))
            .send()
            .map_err(|err| eprintln!("Error getting metrics {:?}", err))
            .and_then(|res| {
                res.text()
                    .map_err(|err| eprintln!("Response was not a string: {:?}", err))
            })
    };

    send_money_to_username(
        node_a_http,
        node_b_http,
        1000,
        "bob_on_b",
        "alice_on_a",
        "token",
    )
    .await
    .unwrap();
    let ret = check_metrics().await.unwrap();
    assert!(ret.starts_with("# metrics snapshot"));
    assert!(ret.contains("requests_incoming_fulfill"));
    assert!(ret.contains("requests_incoming_prepare"));
    assert!(ret.contains("requests_incoming_reject"));
    assert!(ret.contains("requests_incoming_duration"));
    assert!(ret.contains("requests_outgoing_fulfill"));
    assert!(ret.contains("requests_outgoing_prepare"));
    assert!(ret.contains("requests_outgoing_reject"));
    assert!(ret.contains("requests_outgoing_duration"));

    send_money_to_username(
        node_b_http,
        node_a_http,
        2000,
        "alice_on_a",
        "bob_on_b",
        "token",
    )
    .await
    .unwrap();
    let ret = check_metrics().await.unwrap();
    assert!(ret.starts_with("# metrics snapshot"));
    assert!(ret.contains("requests_incoming_fulfill"));
    assert!(ret.contains("requests_incoming_prepare"));
    assert!(ret.contains("requests_incoming_reject"));
    assert!(ret.contains("requests_incoming_duration"));
    assert!(ret.contains("requests_outgoing_fulfill"));
    assert!(ret.contains("requests_outgoing_prepare"));
    assert!(ret.contains("requests_outgoing_reject"));
    assert!(ret.contains("requests_outgoing_duration"));
}
