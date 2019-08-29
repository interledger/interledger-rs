use futures::{stream::Stream, Future};
use interledger_ildcp::IldcpAccount;
use interledger_packet::Address;
use interledger_service::Account as AccountTrait;
use interledger_settlement_engines::engines::ethereum_ledger::run_ethereum_engine;
use interledger_store_redis::Account;
use interledger_store_redis::AccountId;
use redis::ConnectionInfo;
use reqwest;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::HashMap;
use std::fmt::{Debug, Display};
use std::process::Command;
use std::str;
use std::thread::sleep;
use std::time::Duration;
use secrecy::Secret;

#[allow(unused)]
static XRP_FAUCET_URL: &str = "https://faucet.altnet.rippletest.net/accounts";

#[derive(Deserialize)]
pub struct DeliveryData {
    pub delivered_amount: u64,
}

#[derive(Deserialize)]
pub struct BalanceData {
    pub balance: String,
}

#[allow(unused)]
pub fn start_ganache() -> std::process::Child {
    let mut ganache = Command::new("ganache-cli");
    let ganache = ganache.stdout(std::process::Stdio::null()).arg("-m").arg(
        "abstract vacuum mammal awkward pudding scene penalty purchase dinner depart evoke puzzle",
    );
    let ganache_pid = ganache.spawn().expect("couldnt start ganache-cli");
    // wait a couple of seconds for ganache to boot up
    sleep(Duration::from_secs(5));
    ganache_pid
}

#[allow(unused)]
pub fn start_xrp_engine(
    connector_url: &str,
    redis_port: u16,
    engine_port: u16,
    xrp_address: &str,
    xrp_secret: &str,
) -> std::process::Child {
    let mut engine = Command::new("ilp-settlement-xrp");
    engine
        .env("DEBUG", "ilp-settlement-xrp")
        .env("CONNECTOR_URL", connector_url)
        .env("REDIS_PORT", redis_port.to_string())
        .env("ENGINE_PORT", engine_port.to_string())
        .env("LEDGER_ADDRESS", xrp_address)
        .env("LEDGER_SECRET", xrp_secret);
    let engine_pid = engine
        // .stderr(std::process::Stdio::null())
        // .stdout(std::process::Stdio::null())
        .spawn()
        .expect("couldnt start xrp engine");
    sleep(Duration::from_secs(2));
    engine_pid
}

#[allow(unused)]
pub fn start_eth_engine(
    db: ConnectionInfo,
    engine_port: u16,
    key: String,
    settlement_port: u16,
) -> impl Future<Item = (), Error = ()> {
    let key = Secret::new(key);
    run_ethereum_engine(
        db,
        "http://localhost:8545".to_string(),
        engine_port,
        key,
        1,
        0,
        18,
        1000,
        format!("http://127.0.0.1:{}", settlement_port),
        None,
        true,
    )
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
    username: T,
    node_port: u16,
    auth: &str,
) -> impl Future<Item = i64, Error = ()> {
    let client = reqwest::r#async::Client::new();
    client
        .get(&format!(
            "http://localhost:{}/accounts/{}/balance",
            node_port, username
        ))
        .header("Authorization", format!("Bearer {}:{}", username, auth))
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

#[derive(Deserialize, Debug)]
struct FaucetResponse {
    pub account: XrpCredentials,
    pub balance: u64,
}

#[derive(Deserialize, Debug)]
pub struct XrpCredentials {
    pub address: String,
    pub secret: String,
}

#[allow(unused)]
pub fn get_xrp_credentials() -> XrpCredentials {
    // TODO make this async
    let client = reqwest::Client::new();
    let mut res = client
        .post(XRP_FAUCET_URL)
        .send()
        .expect("Unable to get new XRP testnet credentials");
    let body: FaucetResponse = res
        .json()
        .expect("Got unexpected response from XRP testnet faucet");
    body.account
}
