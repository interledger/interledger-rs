use futures::{stream::Stream, Future};
use interledger_packet::Address;
use interledger_service::Account as AccountTrait;
use interledger_store_redis::Account;
use interledger_store_redis::AccountId;
use serde::Serialize;
use serde_json::json;
use std::collections::HashMap;
use std::fmt::{Debug, Display};
use std::str;

#[derive(serde::Deserialize)]
pub struct DeliveryData {
    pub delivered_amount: u64,
}

#[derive(serde::Deserialize)]
pub struct BalanceData {
    pub balance: String,
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
) -> impl Future<Item = u64, Error = ()> {
    let client = reqwest::r#async::Client::new();
    let auth = format!("{}:{}", from_username, from_auth);
    client
        .post(&format!("http://localhost:{}/pay", from_port))
        .header("Authorization", format!("Bearer {}", auth))
        .json(&json!({
            "receiver": format!("http://localhost:{}/spsp/{}", to_port, to_username),
            "source_amount": amount,
        }))
        .send()
        .and_then(|res| res.error_for_status())
        .and_then(|res| res.into_body().concat2())
        .map_err(|err| {
            eprintln!("Error sending SPSP payment: {:?}", err);
        })
        .and_then(move |body| {
            let ret: DeliveryData = serde_json::from_slice(&body).unwrap();
            Ok(ret.delivered_amount)
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
pub fn accounts_to_ids(accounts: Vec<Account>) -> HashMap<Address, AccountId> {
    let mut map = HashMap::new();
    for a in accounts {
        map.insert(a.client_address().clone(), a.id());
    }
    map
}

#[allow(unused)]
pub fn get_balance<T: Display>(
    account_id: T,
    node_port: u16,
    admin_token: &str,
) -> impl Future<Item = i64, Error = ()> {
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
            Ok(ret.balance.parse().unwrap())
        })
}
