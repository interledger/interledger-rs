use futures::TryFutureExt;
use hex;
use interledger::stream::StreamDelivery;
use interledger::{packet::Address, service::Account as AccountTrait, store::account::Account};
use ring::rand::{SecureRandom, SystemRandom};
use serde::Serialize;
use serde_json::json;
use std::collections::HashMap;
use std::fmt::{Debug, Display};
use std::str;
// use tracing_subscriber;
use uuid::Uuid;

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
pub async fn create_account_on_node<T: Serialize>(
    api_port: u16,
    data: T,
    auth: &str,
) -> Result<Account, ()> {
    let client = reqwest::Client::new();
    let res = client
        .post(&format!("http://localhost:{}/accounts", api_port))
        .header("Content-Type", "application/json")
        .header("Authorization", format!("Bearer {}", auth))
        .json(&data)
        .send()
        .map_err(|_| ())
        .await?;

    let res = res.error_for_status().map_err(|_| ())?;

    Ok(res.json::<Account>().map_err(|_| ()).await.unwrap())
}

#[allow(unused)]
pub async fn create_account_on_engine<T: Serialize>(
    engine_port: u16,
    account_id: T,
) -> Result<String, ()> {
    let client = reqwest::Client::new();
    let res = client
        .post(&format!("http://localhost:{}/accounts", engine_port))
        .header("Content-Type", "application/json")
        .json(&json!({ "id": account_id }))
        .send()
        .map_err(|_| ())
        .await?;

    let res = res.error_for_status().map_err(|_| ())?;

    let data: bytes05::Bytes = res.bytes().map_err(|_| ()).await?;

    Ok(str::from_utf8(&data).unwrap().to_string())
}

#[allow(unused)]
pub async fn send_money_to_username<T: Display + Debug>(
    from_port: u16,
    to_port: u16,
    amount: u64,
    to_username: T,
    from_username: &str,
    from_auth: &str,
) -> Result<StreamDelivery, ()> {
    let client = reqwest::Client::new();
    let res = client
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
        .map_err(|_| ())
        .await?;

    let res = res.error_for_status().map_err(|_| ())?;
    Ok(res.json::<StreamDelivery>().await.unwrap())
}

#[allow(unused)]
pub async fn get_all_accounts(node_port: u16, admin_token: &str) -> Result<Vec<Account>, ()> {
    let client = reqwest::Client::new();
    let res = client
        .get(&format!("http://localhost:{}/accounts", node_port))
        .header("Authorization", format!("Bearer {}", admin_token))
        .send()
        .map_err(|_| ())
        .await?;

    let res = res.error_for_status().map_err(|_| ())?;
    let body: bytes05::Bytes = res.bytes().map_err(|_| ()).await?;
    let ret: Vec<Account> = serde_json::from_slice(&body).unwrap();
    Ok(ret)
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
pub async fn get_balance<T: Display>(
    account_id: T,
    node_port: u16,
    admin_token: &str,
) -> Result<BalanceData, ()> {
    let client = reqwest::Client::new();
    let res = client
        .get(&format!(
            "http://localhost:{}/accounts/{}/balance",
            node_port, account_id
        ))
        .header("Authorization", format!("Bearer {}", admin_token))
        .send()
        .map_err(|_| ())
        .await?;

    let res = res.error_for_status().map_err(|_| ())?;
    let body: bytes05::Bytes = res.bytes().map_err(|_| ()).await?;
    let ret: BalanceData = serde_json::from_slice(&body).unwrap();
    Ok(ret)
}
