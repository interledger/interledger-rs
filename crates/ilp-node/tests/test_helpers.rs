use futures::{stream::Stream, Future};
use hex;
use interledger::stream::StreamDelivery;
use interledger::{packet::Address, service::Account as AccountTrait, store::account::Account};
use ring::rand::{SecureRandom, SystemRandom};
use serde::Serialize;
use serde_json::json;
use std::collections::HashMap;
use std::fmt::{Debug, Display};
use std::str;
use tracing_subscriber;
use uuid::Uuid;

pub fn install_tracing_subscriber() {
    tracing_subscriber::fmt::Subscriber::builder()
        .with_timer(tracing_subscriber::fmt::time::ChronoUtc::rfc3339())
        .with_env_filter(tracing_subscriber::filter::EnvFilter::from_default_env())
        .try_init()
        .unwrap_or(());
}

#[allow(unused)]
pub fn random_secret() -> String {
    let mut bytes: [u8; 32] = [0; 32];
    SystemRandom::new().fill(&mut bytes).unwrap();
    hex::encode(bytes)
}

#[derive(serde::Deserialize, Debug, PartialEq)]
pub struct BalanceData {
    pub balance: f64,
    pub asset_code: String,
}

#[allow(unused)]
pub fn create_account_on_node<T: Serialize>(
    api_port: u16,
    data: T,
    auth: &str,
) -> impl Future<Item = String, Error = ()> {
    let client = reqwest::r#async::Client::new();
    client
        .post(&format!("http://localhost:{}/accounts", api_port))
        .header("Content-Type", "application/json")
        .header("Authorization", format!("Bearer {}", auth))
        .json(&data)
        .send()
        .and_then(move |res| res.error_for_status())
        .and_then(move |res| res.into_body().concat2())
        .map_err(|err| {
            eprintln!("Error creating account on node: {:?}", err);
        })
        .and_then(move |chunk| Ok(str::from_utf8(&chunk).unwrap().to_string()))
}

#[allow(unused)]
pub fn create_account_on_engine<T: Serialize>(
    engine_port: u16,
    account_id: T,
) -> impl Future<Item = String, Error = ()> {
    let client = reqwest::r#async::Client::new();
    client
        .post(&format!("http://localhost:{}/accounts", engine_port))
        .header("Content-Type", "application/json")
        .json(&json!({ "id": account_id }))
        .send()
        .and_then(move |res| res.error_for_status())
        .and_then(move |res| res.into_body().concat2())
        .map_err(|err| {
            eprintln!("Error creating account: {:?}", err);
        })
        .and_then(move |chunk| Ok(str::from_utf8(&chunk).unwrap().to_string()))
}

#[allow(unused)]
pub fn send_money_to_username<T: Display + Debug>(
    from_port: u16,
    to_port: u16,
    amount: u64,
    to_username: T,
    from_username: &str,
    from_auth: &str,
) -> impl Future<Item = StreamDelivery, Error = ()> {
    let client = reqwest::r#async::Client::new();
    client
        .post(&format!(
            "http://localhost:{}/accounts/{}/payments",
            from_port, from_username
        ))
        .header("Authorization", format!("Bearer {}", from_auth))
        .json(&json!({
            "receiver": format!("http://localhost:{}/accounts/{}/spsp", to_port, to_username),
            "source_amount": amount,
        }))
        .send()
        .and_then(|res| res.error_for_status())
        .and_then(|res| res.into_body().concat2())
        .map_err(|err| {
            eprintln!("Error sending SPSP payment: {:?}", err);
        })
        .and_then(move |body| {
            let ret: StreamDelivery = serde_json::from_slice(&body).unwrap();
            Ok(ret)
        })
}

#[allow(unused)]
pub fn get_all_accounts(
    node_port: u16,
    admin_token: &str,
) -> impl Future<Item = Vec<Account>, Error = ()> {
    let client = reqwest::r#async::Client::new();
    client
        .get(&format!("http://localhost:{}/accounts", node_port))
        .header("Authorization", format!("Bearer {}", admin_token))
        .send()
        .and_then(|res| res.error_for_status())
        .and_then(|res| res.into_body().concat2())
        .map_err(|err| {
            eprintln!("Error getting account data: {:?}", err);
        })
        .and_then(move |body| {
            let ret: Vec<Account> = serde_json::from_slice(&body).unwrap();
            Ok(ret)
        })
}

#[allow(unused)]
pub fn accounts_to_ids(accounts: Vec<Account>) -> HashMap<Address, Uuid> {
    let mut map = HashMap::new();
    for a in accounts {
        map.insert(a.ilp_address().clone(), a.id());
    }
    map
}

#[allow(unused)]
pub fn get_balance<T: Display>(
    account_id: T,
    node_port: u16,
    admin_token: &str,
) -> impl Future<Item = BalanceData, Error = ()> {
    let client = reqwest::r#async::Client::new();
    client
        .get(&format!(
            "http://localhost:{}/accounts/{}/balance",
            node_port, account_id
        ))
        .header("Authorization", format!("Bearer {}", admin_token))
        .send()
        .and_then(|res| res.error_for_status())
        .and_then(|res| res.into_body().concat2())
        .map_err(|err| {
            eprintln!("Error getting account data: {:?}", err);
        })
        .and_then(|body| {
            let ret: BalanceData = serde_json::from_slice(&body).unwrap();
            Ok(ret)
        })
}
