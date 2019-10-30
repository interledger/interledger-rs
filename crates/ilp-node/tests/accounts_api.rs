mod redis_helpers;
mod test_helpers;

use base64;
use futures::Future;
use redis_helpers::*;
use reqwest;
use serde_json::{json, Number, Value};
use std::str::FromStr;
use std::{thread, time::Duration};
use test_helpers::*;
use tokio::runtime::Builder as RuntimeBuilder;
use warp::http;

use ilp_node::InterledgerNode;
use interledger::{packet::Address, stream::StreamDelivery};

// Integration tests of accounts APIs
// These are very rough tests. It confirms only that the paths and HTTP methods are working correctly.
// TODO add more to make it precise.
// TODO consider using `warp::test` in the cases where we can utilize it
// https://docs.rs/warp/0.1.20/warp/test/index.html

const NODE_ILP_ADDRESS: &str = "example.node";
const ILP_ADDRESS_1: &str = "example.node.1";
const USERNAME_1: &str = "test_account_1";
const USERNAME_2: &str = "test_account_2";
const ASSET_CODE: &str = "XRP";
const ASSET_SCALE: usize = 9;
const MAX_PACKET_AMOUNT: u64 = 100;
const MIN_BALANCE: i64 = -10000;
const ILP_OVER_HTTP_INCOMING_TOKEN_1: &str = "test_1";
const ILP_OVER_HTTP_INCOMING_TOKEN_2: &str = "test_2";
const ASSET_SCALE_TO_BE: usize = 6;
const SETTLE_THRESHOLD_TO_BE: usize = 1000;
const SETTLE_TO_TO_BE: usize = 10;
const ROUTING_RELATION: &str = "NonRoutingAccount";
const ROUND_TRIP_TIME: u32 = 500;
// TODO install redis dependency
// const AMOUNT_PER_MINUTE_LIMIT: u64 = 10000;
// const PACKETS_PER_MINUTE_LIMIT: u32 = 1000;
const SETTLEMENT_ENGINE_URL: &str = "http://localhost:3000/";

#[test]
fn accounts_test() {
    install_tracing_subscriber();

    let mut runtime = RuntimeBuilder::new().build().unwrap();

    let redis_test_context = TestContext::new();
    let node_http_port = get_open_port(Some(7770));
    let node_settlement_port = get_open_port(Some(3000));
    let mut connection_info = redis_test_context.get_client_connection_info();
    connection_info.db = 1;

    let node: InterledgerNode = serde_json::from_value(json!({
        "ilp_address": NODE_ILP_ADDRESS,
        "default_spsp_account": USERNAME_1,
        "admin_auth_token": "admin",
        "redis_connection": connection_info_to_string(connection_info),
        "http_bind_address": format!("127.0.0.1:{}", node_http_port),
        "settlement_api_bind_address": format!("127.0.0.1:{}", node_settlement_port),
        "secret_seed": random_secret(),
        "route_broadcast_interval": 200,
        "exchange_rate_poll_interval": 60000,
        "exchange_rate_poll_failure_tolerance": 5,
    }))
    .unwrap();
    let node_to_serve = node.clone();
    let node_context = move |_| Ok(node);
    let wait_a_sec = |node| {
        thread::sleep(Duration::from_millis(1000));
        Ok(node)
    };

    let get_accounts = move |node: InterledgerNode| {
        // GET /accounts
        println!("Testing: GET /accounts");
        let client = reqwest::r#async::Client::new();
        client
            .get(&format!(
                "http://localhost:{}/accounts",
                node.http_bind_address.port()
            ))
            .header(
                "Authorization",
                &format!("Bearer {}", node.admin_auth_token),
            )
            .send()
            .map_err(|err| panic!(err))
            .and_then(|mut res| {
                let content = res.text().wait().expect("Error getting response!");
                assert!(res.error_for_status_ref().is_ok(), "{}", &content);
                let json: Value = serde_json::from_str(&content)
                    .unwrap_or_else(|_| panic!("Could not parse JSON! JSON: {}", &content));
                assert_eq!(json, Value::Array(vec![]));
                Ok(node)
            })
    };

    let post_accounts_1 = move |node: InterledgerNode| {
        // POST /accounts
        println!("Testing: POST /accounts");
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
                "ilp_address": ILP_ADDRESS_1,
                "username": USERNAME_1,
                "asset_code": ASSET_CODE,
                "asset_scale": ASSET_SCALE,
                "max_packet_amount": MAX_PACKET_AMOUNT,
                "min_balance": MIN_BALANCE,
                "ilp_over_http_incoming_token": ILP_OVER_HTTP_INCOMING_TOKEN_1,
                "routing_relation": ROUTING_RELATION,
                "round_trip_time": ROUND_TRIP_TIME,
                // TODO install redis dependency
                // "amount_per_minute_limit": AMOUNT_PER_MINUTE_LIMIT,
                // "packets_per_minute_limit": PACKETS_PER_MINUTE_LIMIT,
                "settlement_engine_url": SETTLEMENT_ENGINE_URL,
            }))
            .send()
            .map_err(|err| panic!(err))
            .and_then(|mut res| {
                let content = res.text().wait().expect("Error getting response!");
                assert!(res.error_for_status_ref().is_ok(), "{}", &content);
                let json: Value = serde_json::from_str(&content)
                    .unwrap_or_else(|_| panic!("Could not parse JSON! JSON: {}", &content));
                if let Value::Object(account) = json {
                    assert_eq!(
                        account
                            .get("ilp_address")
                            .expect("ilp_address was expected"),
                        &Value::String(ILP_ADDRESS_1.to_owned())
                    );
                    assert_eq!(
                        account.get("username").expect("username was expected"),
                        &Value::String(USERNAME_1.to_owned())
                    );
                    assert_eq!(
                        account.get("asset_code").expect("asset_code was expected"),
                        &Value::String(ASSET_CODE.to_owned())
                    );
                    assert_eq!(
                        account
                            .get("asset_scale")
                            .expect("asset_scale was expected"),
                        &Value::Number(Number::from(ASSET_SCALE))
                    );
                    assert_eq!(
                        account
                            .get("max_packet_amount")
                            .expect("max_packet_amount was expected"),
                        &Value::Number(Number::from(MAX_PACKET_AMOUNT))
                    );
                    assert_eq!(
                        account
                            .get("min_balance")
                            .expect("min_balance was expected"),
                        &Value::Number(Number::from(MIN_BALANCE))
                    );
                    assert_eq!(
                        account
                            .get("routing_relation")
                            .expect("routing_relation was expected"),
                        &Value::String(ROUTING_RELATION.to_owned())
                    );
                    assert_eq!(
                        account
                            .get("round_trip_time")
                            .expect("round_trip_time was expected"),
                        &Value::Number(Number::from(ROUND_TRIP_TIME))
                    );
                    assert_eq!(
                        account
                            .get("settlement_engine_url")
                            .expect("settlement_engine_url was expected"),
                        &Value::String(SETTLEMENT_ENGINE_URL.to_owned())
                    );
                } else {
                    panic!("Invalid response JSON! {}", &content);
                }
                Ok(node)
            })
    };

    let post_accounts_2 = move |node: InterledgerNode| {
        // POST /accounts
        println!("Testing: POST /accounts");
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
                "username": USERNAME_2,
                "asset_code": ASSET_CODE,
                "asset_scale": 12,
                "max_packet_amount": MAX_PACKET_AMOUNT,
                "min_balance": MIN_BALANCE,
                "ilp_over_http_incoming_token": ILP_OVER_HTTP_INCOMING_TOKEN_2,
                "routing_relation": ROUTING_RELATION,
                "round_trip_time": ROUND_TRIP_TIME,
                "settlement_engine_url": SETTLEMENT_ENGINE_URL,
            }))
            .send()
            .map_err(|err| panic!(err))
            .and_then(|mut res| {
                let content = res.text().wait().expect("Error getting response!");
                assert!(res.error_for_status_ref().is_ok(), "{}", &content);
                let json: Value = serde_json::from_str(&content)
                    .unwrap_or_else(|_| panic!("Could not parse JSON! JSON: {}", &content));
                if let Value::Object(account) = json {
                    assert_eq!(
                        account.get("username").expect("username was expected"),
                        &Value::String(USERNAME_2.to_owned())
                    );
                    assert_eq!(
                        account.get("asset_code").expect("asset_code was expected"),
                        &Value::String(ASSET_CODE.to_owned())
                    );
                    assert_eq!(
                        account
                            .get("asset_scale")
                            .expect("asset_scale was expected"),
                        12
                    );
                    assert_eq!(
                        account
                            .get("max_packet_amount")
                            .expect("max_packet_amount was expected"),
                        &Value::Number(Number::from(MAX_PACKET_AMOUNT))
                    );
                    assert_eq!(
                        account
                            .get("min_balance")
                            .expect("min_balance was expected"),
                        &Value::Number(Number::from(MIN_BALANCE))
                    );
                    assert_eq!(
                        account
                            .get("routing_relation")
                            .expect("routing_relation was expected"),
                        &Value::String(ROUTING_RELATION.to_owned())
                    );
                    assert_eq!(
                        account
                            .get("round_trip_time")
                            .expect("round_trip_time was expected"),
                        &Value::Number(Number::from(ROUND_TRIP_TIME))
                    );
                    assert_eq!(
                        account
                            .get("settlement_engine_url")
                            .expect("settlement_engine_url was expected"),
                        &Value::String(SETTLEMENT_ENGINE_URL.to_owned())
                    );
                } else {
                    panic!("Invalid response JSON! {}", &content);
                }
                Ok(node)
            })
    };

    let put_accounts_username = move |node: InterledgerNode| {
        // PUT /accounts/:username
        println!("Testing: PUT /accounts/:username");
        let client = reqwest::r#async::Client::new();
        client
            .put(&format!(
                "http://localhost:{}/accounts/{}",
                node.http_bind_address.port(),
                USERNAME_1
            ))
            .header(
                "Authorization",
                &format!("Bearer {}", node.admin_auth_token),
            )
            .json(&json!({
                "ilp_address": ILP_ADDRESS_1,
                "username": USERNAME_1,
                "asset_code": ASSET_CODE,
                "asset_scale": ASSET_SCALE_TO_BE
            }))
            .send()
            .map_err(|err| panic!(err))
            .and_then(|mut res| {
                let content = res.text().wait().expect("Error getting response!");
                assert!(res.error_for_status_ref().is_ok(), "{}", &content);
                let json: Value = serde_json::from_str(&content)
                    .unwrap_or_else(|_| panic!("Could not parse JSON! JSON: {}", &content));
                if let Value::Object(account) = json {
                    assert_eq!(
                        account
                            .get("ilp_address")
                            .expect("ilp_address was expected"),
                        &Value::String(ILP_ADDRESS_1.to_owned())
                    );
                    assert_eq!(
                        account.get("username").expect("username was expected"),
                        &Value::String(USERNAME_1.to_owned())
                    );
                    assert_eq!(
                        account.get("asset_code").expect("asset_code was expected"),
                        &Value::String(ASSET_CODE.to_owned())
                    );
                    assert_eq!(
                        account
                            .get("asset_scale")
                            .expect("asset_scale was expected"),
                        &Value::Number(Number::from(ASSET_SCALE_TO_BE))
                    );
                } else {
                    panic!("Invalid response JSON! {}", &content);
                }
                Ok(node)
            })
    };

    let get_accounts_username = move |node: InterledgerNode| {
        // GET /accounts/:username
        println!("Testing: GET /accounts/:username");
        let client = reqwest::r#async::Client::new();
        client
            .get(&format!(
                "http://localhost:{}/accounts/{}",
                node.http_bind_address.port(),
                USERNAME_1
            ))
            .header(
                "Authorization",
                &format!("Bearer {}:{}", USERNAME_1, ILP_OVER_HTTP_INCOMING_TOKEN_1),
            )
            .send()
            .map_err(|err| panic!(err))
            .and_then(|mut res| {
                let content = res.text().wait().expect("Error getting response!");
                assert!(res.error_for_status_ref().is_ok(), "{}", &content);
                let json: Value = serde_json::from_str(&content)
                    .unwrap_or_else(|_| panic!("Could not parse JSON! JSON: {}", &content));
                if let Value::Object(account) = json {
                    // Only checks if the specified fields are corret or not because
                    // `put_accounts_username` resets fields which were not specified.
                    assert_eq!(
                        account
                            .get("ilp_address")
                            .expect("ilp_address was expected"),
                        &Value::String(ILP_ADDRESS_1.to_owned())
                    );
                    assert_eq!(
                        account.get("username").expect("username was expected"),
                        &Value::String(USERNAME_1.to_owned())
                    );
                    assert_eq!(
                        account.get("asset_code").expect("asset_code was expected"),
                        &Value::String(ASSET_CODE.to_owned())
                    );
                    assert_eq!(
                        account
                            .get("asset_scale")
                            .expect("asset_scale was expected"),
                        &Value::Number(Number::from(ASSET_SCALE_TO_BE))
                    );
                } else {
                    panic!("Invalid response JSON! {}", &content);
                }
                Ok(node)
            })
    };

    let get_accounts_username_balance = move |node: InterledgerNode| {
        // GET /accounts/:username/balance
        println!("Testing: GET /accounts/:username/balance");
        let client = reqwest::r#async::Client::new();
        client
            .get(&format!(
                "http://localhost:{}/accounts/{}/balance",
                node.http_bind_address.port(),
                USERNAME_1
            ))
            .header(
                "Authorization",
                &format!("Bearer {}:{}", USERNAME_1, ILP_OVER_HTTP_INCOMING_TOKEN_1),
            )
            .send()
            .map_err(|err| panic!(err))
            .and_then(|mut res| {
                let content = res.text().wait().expect("Error getting response!");
                assert!(res.error_for_status_ref().is_ok(), "{}", &content);
                let json: Value = serde_json::from_str(&content)
                    .unwrap_or_else(|_| panic!("Could not parse JSON! JSON: {}", &content));
                if let Value::Object(balance) = json {
                    // TODO why isn't this a number?
                    //assert_eq!(account.get("balance").expect("balance was expected"), &Value::Number(Number::from(0)));
                    assert_eq!(
                        balance.get("balance").expect("balance was expected"),
                        &Value::Number(Number::from(0))
                    );
                } else {
                    panic!("Invalid response JSON! {}", &content);
                }
                Ok(node)
            })
    };

    let put_accounts_username_settings = move |node: InterledgerNode| {
        // PUT /accounts/:username/settings
        println!("Testing: PUT /accounts/:username/settings");
        let client = reqwest::r#async::Client::new();
        client
            .put(&format!(
                "http://localhost:{}/accounts/{}/settings",
                node.http_bind_address.port(),
                USERNAME_1
            ))
            .header(
                "Authorization",
                &format!("Bearer {}:{}", USERNAME_1, ILP_OVER_HTTP_INCOMING_TOKEN_1),
            )
            .json(&json!({
                "settle_threshold": SETTLE_THRESHOLD_TO_BE,
                "settle_to": SETTLE_TO_TO_BE,
            }))
            .send()
            .map_err(|err| panic!(err))
            .and_then(|mut res| {
                let content = res.text().wait().expect("Error getting response!");
                assert!(res.error_for_status_ref().is_ok(), "{}", &content);
                let json: Value = serde_json::from_str(&content)
                    .unwrap_or_else(|_| panic!("Could not parse JSON! JSON: {}", &content));
                if let Value::Object(account) = json {
                    assert_eq!(
                        account
                            .get("settle_threshold")
                            .expect("settle_threshold was expected"),
                        &Value::Number(Number::from(SETTLE_THRESHOLD_TO_BE))
                    );
                    assert_eq!(
                        account.get("settle_to").expect("settle_to was expected"),
                        &Value::Number(Number::from(SETTLE_TO_TO_BE))
                    );
                } else {
                    panic!("Invalid response JSON! {}", &content);
                }
                Ok(node)
            })
    };

    let delete_accounts_username = move |node: InterledgerNode| {
        // DELETE /accounts/:username
        println!("Testing: DELETE /accounts/:username");
        let client = reqwest::r#async::Client::new();
        client
            .delete(&format!(
                "http://localhost:{}/accounts/{}",
                node.http_bind_address.port(),
                USERNAME_1
            ))
            .header(
                "Authorization",
                &format!("Bearer {}", node.admin_auth_token),
            )
            .send()
            .map_err(|err| panic!(err))
            .and_then(|mut res| {
                let content = res.text().wait().expect("Error getting response!");
                assert!(res.error_for_status_ref().is_ok(), "{}", &content);
                let json: Value = serde_json::from_str(&content)
                    .unwrap_or_else(|_| panic!("Could not parse JSON! JSON: {}", &content));
                if let Value::Object(account) = json {
                    assert_eq!(
                        account.get("username").expect("username was expected"),
                        &Value::String(USERNAME_1.to_owned())
                    );
                    assert_eq!(
                        account.get("asset_code").expect("asset_code was expected"),
                        &Value::String(ASSET_CODE.to_owned())
                    );
                    assert_eq!(
                        account
                            .get("asset_scale")
                            .expect("asset_scale was expected"),
                        &Value::Number(Number::from(ASSET_SCALE_TO_BE))
                    );
                } else {
                    panic!("Invalid response JSON! {}", &content);
                }
                Ok(node)
            })
    };

    let post_accounts_username_payments = move |node: InterledgerNode| {
        // POST /accounts/:username/payments
        println!("Testing: POST /accounts/:username/payments");
        let amount = 100;
        let client = reqwest::r#async::Client::new();
        client
            .post(&format!(
                "http://localhost:{}/accounts/{}/payments",
                node.http_bind_address.port(),
                USERNAME_1
            ))
            .header(
                "Authorization",
                &format!("Bearer {}:{}", USERNAME_1, ILP_OVER_HTTP_INCOMING_TOKEN_1),
            )
            .json(&json!({
                    "receiver": &format!("http://localhost:{}/accounts/{}/spsp", node.http_bind_address.port(), USERNAME_2),
                    "source_amount": amount,
                }))
            .send()
            .map_err(|err| panic!(err))
            .and_then(move|mut res| {
                let content = res.text().wait().expect("Error getting response!");
                assert!(res.error_for_status_ref().is_ok(), "{}", &content);
                let receipt: StreamDelivery = serde_json::from_str(&content).unwrap();
                assert_eq!(receipt.from, Address::from_str(ILP_ADDRESS_1).unwrap());
                assert!(receipt.to.to_string().starts_with(&format!("{}.{}", NODE_ILP_ADDRESS, USERNAME_2)));
                assert_eq!(receipt.sent_amount, amount);
                assert_eq!(receipt.sent_asset_scale as usize, ASSET_SCALE);
                assert_eq!(receipt.sent_asset_code, ASSET_CODE);
                assert_eq!(receipt.delivered_amount, amount * 1000);
                assert_eq!(receipt.delivered_asset_scale.unwrap() as usize, 12);
                assert_eq!(receipt.delivered_asset_code.unwrap(), ASSET_CODE);
                Ok(node)
            })
    };

    let get_accounts_username_spsp = move |node: InterledgerNode| {
        // GET /accounts/:username/spsp
        println!("Testing: GET /accounts/:username/spsp");
        let client = reqwest::r#async::Client::new();
        client
            .get(&format!(
                "http://localhost:{}/accounts/{}/spsp",
                node.http_bind_address.port(),
                USERNAME_1
            ))
            .send()
            .map_err(|err| panic!(err))
            .and_then(|mut res| {
                let content = res.text().wait().expect("Error getting response!");
                assert!(res.error_for_status_ref().is_ok(), "{}", &content);
                let json: Value = serde_json::from_str(&content)
                    .unwrap_or_else(|_| panic!("Could not parse JSON! JSON: {}", &content));
                if let Value::Object(spsp) = json {
                    if let Value::String(account) = spsp
                        .get("destination_account")
                        .expect("destination_account was expected")
                    {
                        assert!(
                            account.starts_with(ILP_ADDRESS_1),
                            "destination_account doesn't start with address of {}",
                            USERNAME_1
                        );
                    } else {
                        panic!("Invalid response JSON! {}", &content);
                    }
                    if let Value::String(shared_secret) = spsp
                        .get("shared_secret")
                        .expect("shared_secret was expected")
                    {
                        let shared_secret_bytes = base64::decode(&shared_secret)
                            .expect("shared_secret is expected to be BASE64.");
                        assert_eq!(
                            shared_secret_bytes.len(),
                            32,
                            "shared_secret was not 32bytes!"
                        );
                    } else {
                        panic!("Invalid response JSON! {}", &content);
                    }
                } else {
                    panic!("Invalid response JSON! {}", &content);
                }
                Ok(node)
            })
    };

    let get_well_known_pay = move |node: InterledgerNode| {
        // GET /.well-known/pay
        println!("Testing: GET /.well-known/pay");
        let client = reqwest::r#async::Client::new();
        client
            .get(&format!(
                "http://localhost:{}/.well-known/pay",
                node.http_bind_address.port()
            ))
            .send()
            .map_err(|err| panic!(err))
            .and_then(|mut res| {
                let content = res.text().wait().expect("Error getting response!");
                assert!(res.error_for_status_ref().is_ok(), "{}", &content);
                let json: Value = serde_json::from_str(&content)
                    .unwrap_or_else(|_| panic!("Could not parse JSON! JSON: {}", &content));
                if let Value::Object(spsp) = json {
                    if let Value::String(account) = spsp
                        .get("destination_account")
                        .expect("destination_account was expected")
                    {
                        assert!(
                            account.starts_with(ILP_ADDRESS_1),
                            "destination_account doesn't start with address of {}",
                            USERNAME_1
                        );
                    } else {
                        panic!("Invalid response JSON! {}", &content);
                    }
                    if let Value::String(shared_secret) = spsp
                        .get("shared_secret")
                        .expect("shared_secret was expected")
                    {
                        let shared_secret_bytes = base64::decode(&shared_secret)
                            .expect("shared_secret is expected to be BASE64.");
                        assert_eq!(
                            shared_secret_bytes.len(),
                            32,
                            "shared_secret was not 32bytes!"
                        );
                    } else {
                        panic!("Invalid response JSON! {}", &content);
                    }
                } else {
                    panic!("Invalid response JSON! {}", &content);
                }
                Ok(node)
            })
    };

    let get_accounts_username_by_admin = move |node: InterledgerNode| {
        // GET /accounts/:username
        println!("Testing: GET /accounts/:username [ADMIN]");
        let client = reqwest::r#async::Client::new();
        client
            .get(&format!(
                "http://localhost:{}/accounts/{}",
                node.http_bind_address.port(),
                USERNAME_1
            ))
            .header(
                "Authorization",
                &format!("Bearer {}", node.admin_auth_token),
            )
            .send()
            .map_err(|err| panic!(err))
            .and_then(|mut res| {
                let content = res.text().wait().expect("Error getting response!");
                assert!(res.error_for_status_ref().is_ok(), "{}", &content);
                let json: Value = serde_json::from_str(&content)
                    .unwrap_or_else(|_| panic!("Could not parse JSON! JSON: {}", &content));
                if let Value::Object(account) = json {
                    // Only checks if the specified fields are corret or not because
                    // `put_accounts_username` resets fields which were not specified.
                    assert_eq!(
                        account
                            .get("ilp_address")
                            .expect("ilp_address was expected"),
                        &Value::String(ILP_ADDRESS_1.to_owned())
                    );
                    assert_eq!(
                        account.get("username").expect("username was expected"),
                        &Value::String(USERNAME_1.to_owned())
                    );
                    assert_eq!(
                        account.get("asset_code").expect("asset_code was expected"),
                        &Value::String(ASSET_CODE.to_owned())
                    );
                    assert_eq!(
                        account
                            .get("asset_scale")
                            .expect("asset_scale was expected"),
                        &Value::Number(Number::from(ASSET_SCALE))
                    );
                } else {
                    panic!("Invalid response JSON! {}", &content);
                }
                Ok(node)
            })
    };

    // Should cause Unauthorized
    let get_accounts_username_unauthorized = move |node: InterledgerNode| {
        // GET /accounts/:username
        println!("Testing: GET /accounts/:username");
        let client = reqwest::r#async::Client::new();
        client
            .get(&format!(
                "http://localhost:{}/accounts/{}",
                node.http_bind_address.port(),
                USERNAME_1
            ))
            .header(
                "Authorization",
                &format!("Bearer {}:{}", USERNAME_2, ILP_OVER_HTTP_INCOMING_TOKEN_2),
            )
            .send()
            .map_err(|err| panic!(err))
            .and_then(|mut res| {
                let content = res.text().wait().expect("Error getting response!");
                let json: Value = serde_json::from_str(&content)
                    .unwrap_or_else(|_| panic!("Could not parse JSON! JSON: {}", &content));
                if let Value::Object(account) = json {
                    assert_eq!(
                        account.get("status").expect("status was expected"),
                        &Value::Number(Number::from(http::StatusCode::UNAUTHORIZED.as_u16()))
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
                .and_then(get_accounts)
                .and_then(post_accounts_1)
                .and_then(put_accounts_username)
                .and_then(get_accounts_username)
                .and_then(get_accounts_username_balance)
                .and_then(put_accounts_username_settings)
                .and_then(delete_accounts_username)
                .and_then(post_accounts_1)
                .and_then(post_accounts_2)
                .and_then(wait_a_sec) // Seems that we need to wait a while after the account insertions.
                .and_then(post_accounts_username_payments)
                .and_then(get_accounts_username_spsp)
                .and_then(get_well_known_pay)
                .and_then(get_accounts_username_by_admin)
                .and_then(get_accounts_username_unauthorized),
        )
        .expect("Could not spin up node and tests.");
}
