use env_logger;
use futures::Future;
use interledger::{
    cli,
    node::{ExchangeRateProvider, InterledgerNode},
};
use interledger_packet::Address;
use interledger_service::Username;
use reqwest::r#async::Client;
use serde_json::Value;
use std::str::FromStr;
use tokio::runtime::Builder as RuntimeBuilder;

mod redis_helpers;
use redis_helpers::*;

#[test]
fn coincap() {
    let _ = env_logger::try_init();
    let context = TestContext::new();

    let mut runtime = RuntimeBuilder::new()
        .panic_handler(|_| panic!("Tokio worker panicked"))
        .build()
        .unwrap();

    let http_port = get_open_port(Some(3010));

    let node = InterledgerNode {
        ilp_address: Address::from_str("example.one").unwrap(),
        default_spsp_account: Some(Username::from_str("one").unwrap()),
        admin_auth_token: "admin".to_string(),
        redis_connection: context.get_client_connection_info(),
        btp_bind_address: ([127, 0, 0, 1], get_open_port(None)).into(),
        http_bind_address: ([127, 0, 0, 1], http_port).into(),
        settlement_api_bind_address: ([127, 0, 0, 1], get_open_port(Some(3011))).into(),
        secret_seed: cli::random_secret(),
        route_broadcast_interval: Some(200),
        exchange_rate_poll_interval: 60000,
        exchange_rate_provider: Some(ExchangeRateProvider::CoinCap),
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
