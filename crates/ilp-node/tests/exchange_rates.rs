use futures::Future;
use ilp_node::InterledgerNode;
use reqwest::r#async::Client;
use secrecy::SecretString;
use serde_json::{self, json, Value};
use std::env;
use tokio::runtime::Builder as RuntimeBuilder;
use tokio_retry::{strategy::FibonacciBackoff, Retry};
use tracing::error;
use tracing_subscriber;

mod redis_helpers;
use redis_helpers::*;
mod test_helpers;
use test_helpers::*;

#[test]
fn coincap() {
    install_tracing_subscriber();
    let context = TestContext::new();

    let mut runtime = RuntimeBuilder::new()
        .panic_handler(|err| std::panic::resume_unwind(err))
        .build()
        .unwrap();

    let http_port = get_open_port(Some(3010));

    let node: InterledgerNode = serde_json::from_value(json!({
        "ilp_address": "example.one",
        "default_spsp_account": "one",
        "admin_auth_token": "admin",
        "redis_connection": connection_info_to_string(context.get_client_connection_info()),
        "http_bind_address": format!("127.0.0.1:{}", http_port),
        "settlement_api_bind_address": format!("127.0.0.1:{}", get_open_port(None)),
        "secret_seed": random_secret(),
        "route_broadcast_interval": 200,
        "exchange_rate_poll_interval": 60000,
        "exchange_rate_provider": "CoinCap",
    }))
    .unwrap();
    runtime.spawn(node.serve());

    let get_rates = move || {
        Client::new()
            .get(&format!("http://localhost:{}/rates", http_port))
            .send()
            .map_err(|_| panic!("Error getting rates"))
            .and_then(|mut res| res.json().map_err(|_| panic!("Error getting body")))
            .and_then(|body: Value| {
                if let Value::Object(obj) = body {
                    if obj.is_empty() {
                        error!("Rates are empty");
                        return Err(());
                    }
                    assert_eq!(
                        format!("{}", obj.get("USD").expect("Should have USD rate")).as_str(),
                        "1.0"
                    );
                    assert!(obj.contains_key("EUR"));
                    assert!(obj.contains_key("JPY"));
                    assert!(obj.contains_key("BTC"));
                    assert!(obj.contains_key("ETH"));
                    assert!(obj.contains_key("XRP"));
                } else {
                    panic!("Not an object");
                }

                Ok(())
            })
    };

    runtime
        .block_on(
            delay(1000)
                .map_err(|_| panic!("Something strange happened"))
                .and_then(move |_| {
                    Retry::spawn(FibonacciBackoff::from_millis(1000).take(5), get_rates)
                }),
        )
        .unwrap();
}

// TODO can we disable this with conditional compilation?
#[test]
fn cryptocompare() {
    tracing_subscriber::fmt::try_init().unwrap_or(());
    let context = TestContext::new();

    let api_key = env::var("ILP_TEST_CRYPTOCOMPARE_API_KEY");
    if api_key.is_err() {
        error!("Skipping cryptocompare test. Must configure an API key by setting ILP_TEST_CRYPTOCOMPARE_API_KEY to run this test");
        return;
    }
    let api_key = SecretString::new(api_key.unwrap());

    let mut runtime = RuntimeBuilder::new()
        .panic_handler(|err| std::panic::resume_unwind(err))
        .build()
        .unwrap();

    let http_port = get_open_port(Some(3011));

    let node: InterledgerNode = serde_json::from_value(json!({
        "ilp_address": "example.one",
        "default_spsp_account": "one",
        "admin_auth_token": "admin",
        "redis_connection": connection_info_to_string(context.get_client_connection_info()),
        "http_bind_address": format!("127.0.0.1:{}", http_port),
        "settlement_api_bind_address": format!("127.0.0.1:{}", get_open_port(None)),
        "secret_seed": random_secret(),
        "route_broadcast_interval": 200,
        "exchange_rate_poll_interval": 60000,
        "exchange_rate_provider": {
            "crypto_compare": api_key
        },
        "exchange_rate_spread": 0.0,
    }))
    .unwrap();
    runtime.spawn(node.serve());

    let get_rates = move || {
        Client::new()
            .get(&format!("http://localhost:{}/rates", http_port))
            .send()
            .map_err(|_| panic!("Error getting rates"))
            .and_then(|mut res| res.json().map_err(|_| panic!("Error getting body")))
            .and_then(|body: Value| {
                if let Value::Object(obj) = body {
                    if obj.is_empty() {
                        error!("Rates are empty");
                        return Err(());
                    }
                    assert_eq!(
                        format!("{}", obj.get("USD").expect("Should have USD rate")).as_str(),
                        "1.0"
                    );
                    assert!(obj.contains_key("BTC"));
                    assert!(obj.contains_key("ETH"));
                    assert!(obj.contains_key("XRP"));
                } else {
                    panic!("Not an object");
                }

                Ok(())
            })
    };

    runtime
        .block_on(
            delay(1000)
                .map_err(|_| panic!("Something strange happened"))
                .and_then(move |_| {
                    Retry::spawn(FibonacciBackoff::from_millis(1000).take(5), get_rates)
                }),
        )
        .unwrap();
}
