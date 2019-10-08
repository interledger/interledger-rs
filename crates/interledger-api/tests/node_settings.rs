mod common;

use approx::relative_eq;
use common::redis_test_helpers;
use futures::Future;
use reqwest::{self, r#async::Body};
use serde_json::{json, Value};
use std::str::FromStr;
use tokio::runtime::Builder as RuntimeBuilder;

use ilp_node::{random_secret, InterledgerNode};
use interledger::packet::Address;

// Integration tests of node settings APIs

const NODE_ILP_ADDRESS: &str = "example.node";
const USERNAME: &str = "test_account";
const ASSET_CODE: &str = "XRP";
const ASSET_SCALE: usize = 9;
const XRP_RATE: f64 = 1.0;
const ETH_RATE: f64 = 1.2;
const SETTLEMENT_ENGINE_ASSET_CODE: &str = "XRP";
const SETTLEMENT_ENGINE_URL: &str = "http://localhost:3000/";

#[test]
fn node_settings_test() {
    let _ = env_logger::try_init();

    let mut runtime = RuntimeBuilder::new().build().unwrap();

    let redis_test_context = redis_test_helpers::TestContext::new();
    let node_http_port = redis_test_helpers::get_open_port(Some(7770));
    let node_settlement_port = redis_test_helpers::get_open_port(Some(3000));
    let mut connection_info = redis_test_context.get_client_connection_info();
    connection_info.db = 1;

    let node = InterledgerNode {
        ilp_address: Some(
            Address::from_str(NODE_ILP_ADDRESS).expect("Could not parse ILP address."),
        ),
        default_spsp_account: None,
        admin_auth_token: "admin".to_string(),
        redis_connection: connection_info,
        http_bind_address: ([127, 0, 0, 1], node_http_port).into(),
        settlement_api_bind_address: ([127, 0, 0, 1], node_settlement_port).into(),
        secret_seed: random_secret(),
        route_broadcast_interval: Some(200),
        exchange_rate_poll_interval: 60000,
        exchange_rate_provider: None,
        exchange_rate_spread: 0.0,
    };
    let node_to_serve = node.clone();
    let node_context = move |_| Ok(node);

    let get_root = move |node: InterledgerNode| {
        // GET / (stat)
        let client = reqwest::r#async::Client::new();
        client
            .get(&format!(
                "http://localhost:{}/",
                node.http_bind_address.port()
            ))
            .send()
            .map_err(|err| panic!(err))
            .and_then(|mut res| {
                let content = res.text().wait().expect("Error getting response!");
                assert!(res.error_for_status_ref().is_ok(), "{}", &content);
                let json: Value = serde_json::from_str(&content)
                    .expect(&format!("Could not parse JSON! JSON: {}", &content));
                if let Value::Object(account) = json {
                    assert_eq!(
                        account.get("status").expect("status was expected"),
                        &Value::String("Ready".to_owned())
                    );
                    assert_eq!(
                        account
                            .get("ilp_address")
                            .expect("ilp_address was expected"),
                        &Value::String(
                            node.ilp_address
                                .clone()
                                .expect("ILP address was expected")
                                .to_string()
                        )
                    );
                } else {
                    panic!("Invalid response JSON! {}", &content);
                }
                Ok(node)
            })
    };

    let put_rates = move |node: InterledgerNode| {
        // PUT /rates
        let client = reqwest::r#async::Client::new();
        client
            .put(&format!(
                "http://localhost:{}/rates",
                node.http_bind_address.port()
            ))
            .header(
                "Authorization",
                &format!("Bearer {}", node.admin_auth_token),
            )
            .json(&json!({
                "XRP": XRP_RATE,
                "ETH": ETH_RATE,
            }))
            .send()
            .map_err(|err| panic!(err))
            .and_then(move |mut res| {
                let content = res.text().wait().expect("Error getting response!");
                assert!(res.error_for_status_ref().is_ok(), "{}", &content);
                let json: Value = serde_json::from_str(&content)
                    .expect(&format!("Could not parse JSON! JSON: {}", &content));
                if let Value::Object(rates) = json {
                    if let Value::Number(num) = rates.get("XRP").expect("XRP was expected") {
                        assert!(relative_eq!(
                            XRP_RATE,
                            num.as_f64().expect("rate is expected to be f64"),
                            epsilon = std::f64::EPSILON
                        ));
                    } else {
                        panic!("Invalid response JSON! {}", &content);
                    }
                    if let Value::Number(num) = rates.get("ETH").expect("ETH was expected") {
                        assert!(relative_eq!(
                            ETH_RATE,
                            num.as_f64().expect("rate is expected to be f64"),
                            epsilon = std::f64::EPSILON
                        ));
                    } else {
                        panic!("Invalid response JSON! {}", &content);
                    }
                } else {
                    panic!("Invalid response JSON! {}", &content);
                }
                Ok(node)
            })
    };

    let get_rates = move |node: InterledgerNode| {
        // GET /rates
        let client = reqwest::r#async::Client::new();
        client
            .get(&format!(
                "http://localhost:{}/rates",
                node.http_bind_address.port()
            ))
            .send()
            .map_err(|err| panic!(err))
            .and_then(move |mut res| {
                let content = res.text().wait().expect("Error getting response!");
                assert!(res.error_for_status_ref().is_ok(), "{}", &content);
                let json: Value = serde_json::from_str(&content)
                    .expect(&format!("Could not parse JSON! JSON: {}", &content));
                if let Value::Object(rates) = json {
                    if let Value::Number(num) = rates.get("XRP").expect("XRP was expected") {
                        assert!(relative_eq!(
                            XRP_RATE,
                            num.as_f64().expect("rate is expected to be f64"),
                            epsilon = std::f64::EPSILON
                        ));
                    } else {
                        panic!("Invalid response JSON! {}", &content);
                    }
                    if let Value::Number(num) = rates.get("ETH").expect("ETH was expected") {
                        assert!(relative_eq!(
                            ETH_RATE,
                            num.as_f64().expect("rate is expected to be f64"),
                            epsilon = std::f64::EPSILON
                        ));
                    } else {
                        panic!("Invalid response JSON! {}", &content);
                    }
                } else {
                    panic!("Invalid response JSON! {}", &content);
                }
                Ok(node)
            })
    };

    let get_routes = move |node: InterledgerNode| {
        // GET /routes
        let client = reqwest::r#async::Client::new();
        client
            .get(&format!(
                "http://localhost:{}/routes",
                node.http_bind_address.port()
            ))
            .send()
            .map_err(|err| panic!(err))
            .and_then(move |mut res| {
                let content = res.text().wait().expect("Error getting response!");
                assert!(res.error_for_status_ref().is_ok(), "{}", &content);
                let json: Value = serde_json::from_str(&content)
                    .expect(&format!("Could not parse JSON! JSON: {}", &content));
                if let Value::Object(_rates) = json {
                    // nothing so far
                } else {
                    panic!("Invalid response JSON! {}", &content);
                }
                Ok(node)
            })
    };

    // This is not a part of the tests. This is tested in `accounts.rs`.
    let post_accounts = move |node: InterledgerNode| {
        // POST /accounts
        let client = reqwest::r#async::Client::new();
        client
            .post(&format!(
                "http://localhost:{}/accounts",
                node.http_bind_address.port()
            ))
            .header(
                "Authorization",
                &format!("Bearer {}", node.admin_auth_token),
            )
            .json(&json!({
                "username": USERNAME,
                "asset_code": ASSET_CODE,
                "asset_scale": ASSET_SCALE
            }))
            .send()
            .map_err(|err| panic!(err))
            .and_then(|mut res| {
                let content = res.text().wait().expect("Error getting response!");
                assert!(res.error_for_status_ref().is_ok(), "{}", &content);
                let json: Value = serde_json::from_str(&content)
                    .expect(&format!("Could not parse JSON! JSON: {}", &content));
                if let Value::Object(account) = json {
                    let account_id = account
                        .get("id")
                        .map(|value| match value {
                            Value::String(string) => string,
                            _ => panic!("Invalid response JSON! {}", &content),
                        })
                        .expect("id was expected");
                    let ilp_address = account
                        .get("ilp_address")
                        .map(|value| match value {
                            Value::String(string) => string,
                            _ => panic!("Invalid response JSON! {}", &content),
                        })
                        .expect("ilp_address was expected");
                    Ok((node, account_id.to_owned(), ilp_address.to_owned()))
                } else {
                    panic!("Invalid response JSON! {}", &content);
                }
            })
    };

    let put_routes_static =
        move |(node, account_id, ilp_address): (InterledgerNode, String, String)| {
            // PUT /routes/static
            let client = reqwest::r#async::Client::new();
            client
                .put(&format!(
                    "http://localhost:{}/routes/static",
                    node.http_bind_address.port()
                ))
                .header(
                    "Authorization",
                    &format!("Bearer {}", node.admin_auth_token),
                )
                .json(&json!({
                    "example.a": &account_id,
                    "example.b": &account_id,
                    "example.c": &account_id,
                }))
                .send()
                .map_err(|err| panic!(err))
                .and_then(move |mut res| {
                    let content = res.text().wait().expect("Error getting response!");
                    assert!(res.error_for_status_ref().is_ok(), "{}", &content);
                    let json: Value = serde_json::from_str(&content)
                        .expect(&format!("Could not parse JSON! JSON: {}", &content));
                    if let Value::Object(account) = json {
                        assert_eq!(
                            account.get("example.a").expect("example.a was expected"),
                            &Value::String(account_id.to_owned())
                        );
                        assert_eq!(
                            account.get("example.b").expect("example.b was expected"),
                            &Value::String(account_id.to_owned())
                        );
                        assert_eq!(
                            account.get("example.c").expect("example.c was expected"),
                            &Value::String(account_id.to_owned())
                        );
                    } else {
                        panic!("Invalid response JSON! {}", &content);
                    }
                    Ok((node, account_id, ilp_address))
                })
        };

    let put_routes_static_prefix =
        move |(node, account_id, ilp_address): (InterledgerNode, String, String)| {
            // PUT /routes/static/:prefix
            let client = reqwest::r#async::Client::new();
            client
                .put(&format!(
                    "http://localhost:{}/routes/static/{}",
                    node.http_bind_address.port(),
                    ilp_address
                ))
                .header(
                    "Authorization",
                    &format!("Bearer {}", node.admin_auth_token),
                )
                .body(Body::from(account_id.clone()))
                .send()
                .map_err(|err| panic!(err))
                .and_then(move |mut res| {
                    let content = res.text().wait().expect("Error getting response!");
                    assert!(res.error_for_status_ref().is_ok(), "{}", &content);
                    assert_eq!(content, account_id);
                    Ok(node)
                })
        };

    // The API can't find the settlement engine actually but it is OK because
    // this is just testing if the API correctly accepts the requests or not.
    let put_settlement_engines = move |node: InterledgerNode| {
        // PUT /settlement/engines
        let client = reqwest::r#async::Client::new();
        client
            .put(&format!(
                "http://localhost:{}/settlement/engines",
                node.http_bind_address.port()
            ))
            .header(
                "Authorization",
                &format!("Bearer {}", node.admin_auth_token),
            )
            .json(&json!({
                SETTLEMENT_ENGINE_ASSET_CODE: SETTLEMENT_ENGINE_URL,
            }))
            .send()
            .map_err(|err| panic!(err))
            .and_then(move |mut res| {
                let content = res.text().wait().expect("Error getting response!");
                assert!(res.error_for_status_ref().is_ok(), "{}", &content);
                let json: Value = serde_json::from_str(&content)
                    .expect(&format!("Could not parse JSON! JSON: {}", &content));
                if let Value::Object(account) = json {
                    assert_eq!(
                        account
                            .get(SETTLEMENT_ENGINE_ASSET_CODE)
                            .expect(&format!("{} was expected", SETTLEMENT_ENGINE_ASSET_CODE)),
                        &Value::String(SETTLEMENT_ENGINE_URL.to_owned())
                    );
                } else {
                    panic!("Invalid response JSON! {}", &content);
                }
                Ok(node)
            })
    };

    runtime
        .block_on(
            node_to_serve
                .serve()
                .and_then(node_context)
                .and_then(get_root)
                .and_then(put_rates)
                .and_then(get_rates)
                .and_then(get_routes)
                .and_then(post_accounts)
                .and_then(put_routes_static)
                .and_then(put_routes_static_prefix)
                .and_then(put_settlement_engines),
        )
        .expect("Could not spin up node and tests.");
}
