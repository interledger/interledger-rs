use env_logger;
use futures::Future;
use ilp_node::{random_secret, ExchangeRateProvider, InterledgerNode};
use interledger::{packet::Address, service::Username};
use log::error;
use reqwest::r#async::Client;
use secrecy::SecretString;
use serde_json::Value;
use std::env;
use std::str::FromStr;
use tokio::runtime::Builder as RuntimeBuilder;

mod redis_helpers;
use redis_helpers::*;

//#[test]
fn coincap() {
    let _ = env_logger::try_init();
    let context = TestContext::new();

    let mut runtime = RuntimeBuilder::new()
        .panic_handler(|_| panic!("Tokio worker panicked"))
        .build()
        .unwrap();

    let http_port = get_open_port(Some(3010));

    let node = InterledgerNode {
        ilp_address: Some(Address::from_str("example.one").unwrap()),
        default_spsp_account: Some(Username::from_str("one").unwrap()),
        admin_auth_token: "admin".to_string(),
        redis_connection: context.get_client_connection_info(),
        http_bind_address: ([127, 0, 0, 1], http_port).into(),
        settlement_api_bind_address: ([127, 0, 0, 1], get_open_port(None)).into(),
        secret_seed: random_secret(),
        route_broadcast_interval: Some(200),
        exchange_rate_poll_interval: 60000,
        exchange_rate_provider: Some(ExchangeRateProvider::CoinCap),
        exchange_rate_spread: 0.0,
    };
    runtime.spawn(node.serve());

    runtime
        .block_on(
            delay(1000)
                .map_err(|_| panic!("Something strange happened"))
                .and_then(move |_| {
                    Client::new()
                        .get(&format!("http://localhost:{}/rates", http_port))
                        .send()
                        .map_err(|_| panic!("Error getting rates"))
                        .and_then(|mut res| res.json().map_err(|_| panic!("Error getting body")))
                        .and_then(|body: Value| {
                            if let Value::Object(obj) = body {
                                assert_eq!(
                                    format!("{}", obj.get("USD").expect("Should have USD rate"))
                                        .as_str(),
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
                }),
        )
        .unwrap();
}

// TODO can we disable this with conditional compilation?
//#[test]
fn cryptocompare() {
    let _ = env_logger::try_init();
    let context = TestContext::new();

    let api_key = env::var("ILP_TEST_CRYPTOCOMPARE_API_KEY");
    if api_key.is_err() {
        error!("Skipping cryptocompare test. Must configure an API key by setting ILP_TEST_CRYPTOCOMPARE_API_KEY to run this test");
        return;
    }
    let api_key = SecretString::new(api_key.unwrap());

    let mut runtime = RuntimeBuilder::new()
        .panic_handler(|_| panic!("Tokio worker panicked"))
        .build()
        .unwrap();

    let http_port = get_open_port(Some(3011));

    let node = InterledgerNode {
        ilp_address: Some(Address::from_str("example.one").unwrap()),
        default_spsp_account: Some(Username::from_str("one").unwrap()),
        admin_auth_token: "admin".to_string(),
        redis_connection: context.get_client_connection_info(),
        http_bind_address: ([127, 0, 0, 1], http_port).into(),
        settlement_api_bind_address: ([127, 0, 0, 1], get_open_port(None)).into(),
        secret_seed: random_secret(),
        route_broadcast_interval: Some(200),
        exchange_rate_poll_interval: 60000,
        exchange_rate_provider: Some(ExchangeRateProvider::CryptoCompare(api_key)),
        exchange_rate_spread: 0.0,
    };
    runtime.spawn(node.serve());

    runtime
        .block_on(
            delay(1000)
                .map_err(|_| panic!("Something strange happened"))
                .and_then(move |_| {
                    Client::new()
                        .get(&format!("http://localhost:{}/rates", http_port))
                        .send()
                        .map_err(|_| panic!("Error getting rates"))
                        .and_then(|mut res| res.json().map_err(|_| panic!("Error getting body")))
                        .and_then(|body: Value| {
                            if let Value::Object(obj) = body {
                                assert_eq!(
                                    format!("{}", obj.get("USD").expect("Should have USD rate"))
                                        .as_str(),
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
                }),
        )
        .unwrap();
}
