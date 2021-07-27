use crate::redis_helpers::*;
use crate::test_helpers::*;
use ilp_node::InterledgerNode;
use reqwest::Client;
use secrecy::SecretString;
use serde_json::{self, json, Value};
use std::env;
use std::time::Duration;
use tracing::error;

#[tokio::test]
async fn coincap() {
    let context = TestContext::new();

    let http_port = get_open_port(None);

    let node: InterledgerNode = serde_json::from_value(json!({
        "ilp_address": "example.one",
        "default_spsp_account": "one",
        "admin_auth_token": "admin",
        "database_url": connection_info_to_string(context.get_client_connection_info()),
        "http_bind_address": format!("127.0.0.1:{}", http_port),
        "settlement_api_bind_address": format!("127.0.0.1:{}", get_open_port(None)),
        "secret_seed": random_secret(),
        "route_broadcast_interval": 200,
        "exchange_rate": {
            "poll_interval": 100,
            "provider": "coincap",
        },
    }))
    .unwrap();
    node.serve(None).await.unwrap();

    // Wait a few seconds so our node can poll the API
    tokio::time::sleep(Duration::from_millis(1000)).await;

    let ret = Client::new()
        .get(&format!("http://localhost:{}/rates", http_port))
        .send()
        .await
        .unwrap();
    let txt = ret.text().await.unwrap();
    let obj: Value = serde_json::from_str(&txt).unwrap();

    assert_eq!(
        format!("{}", obj.get("USD").expect("Should have USD rate")).as_str(),
        "1.0"
    );

    // since coinbase sometimes suspends some exchange rates we would consider the test correct if 70% of the following rates are available
    let mut count = 0;
    let expected_rates = ["EUR", "JPY", "BTC", "ETH", "XRP"];
    for r in &expected_rates {
        if obj.get(r).is_some() {
            count += 1;
        }
    }

    assert!(count as f32 >= 0.7 * expected_rates.len() as f32)
}

// TODO can we disable this with conditional compilation?
#[tokio::test]
async fn cryptocompare() {
    let context = TestContext::new();

    let api_key = env::var("ILP_TEST_CRYPTOCOMPARE_API_KEY");
    if api_key.is_err() {
        error!("Skipping cryptocompare test. Must configure an API key by setting ILP_TEST_CRYPTOCOMPARE_API_KEY to run this test");
        return;
    }
    let api_key = SecretString::new(api_key.unwrap());

    let http_port = get_open_port(Some(3011));

    let node: InterledgerNode = serde_json::from_value(json!({
        "ilp_address": "example.one",
        "default_spsp_account": "one",
        "admin_auth_token": "admin",
        "database_url": connection_info_to_string(context.get_client_connection_info()),
        "http_bind_address": format!("127.0.0.1:{}", http_port),
        "settlement_api_bind_address": format!("127.0.0.1:{}", get_open_port(None)),
        "secret_seed": random_secret(),
        "route_broadcast_interval": 200,
        "exchange_rate": {
            "poll_interval": 100,
            "provider": {
                "cryptocompare": api_key
            },
            "spread": 0.0,
        },
    }))
    .unwrap();
    node.serve(None).await.unwrap();

    // Wait a few seconds so our node can poll the API
    tokio::time::sleep(Duration::from_millis(1000)).await;

    let ret = Client::new()
        .get(&format!("http://localhost:{}/rates", http_port))
        .send()
        .await
        .unwrap();
    let txt = ret.text().await.unwrap();
    let obj: Value = serde_json::from_str(&txt).unwrap();

    assert_eq!(
        format!("{}", obj.get("USD").expect("Should have USD rate")).as_str(),
        "1.0"
    );
    assert!(obj.get("BTC").is_some());
    assert!(obj.get("ETH").is_some());
    assert!(obj.get("XRP").is_some());
}
