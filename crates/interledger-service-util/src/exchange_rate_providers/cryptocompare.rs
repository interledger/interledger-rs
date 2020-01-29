use futures::TryFutureExt;
use lazy_static::lazy_static;
use log::error;
use reqwest::{Client, Url};
use secrecy::{ExposeSecret, SecretString};
use serde::Deserialize;
use std::{
    collections::HashMap,
    iter::{once, FromIterator},
};

lazy_static! {
    static ref CRYPTOCOMPARE_URL: Url =
        Url::parse("https://min-api.cryptocompare.com/data/top/mktcapfull?limit=100&tsym=USD")
            .unwrap();
}

#[derive(Deserialize, Debug)]
struct Price {
    #[serde(rename = "PRICE")]
    price: f64,
}

#[derive(Deserialize, Debug)]
struct Raw {
    #[serde(rename = "USD")]
    usd: Price,
}

#[derive(Deserialize, Debug)]
struct CoinInfo {
    #[serde(rename(deserialize = "Name"))]
    name: String,
}

#[derive(Deserialize, Debug)]
struct Record {
    #[serde(rename = "CoinInfo")]
    coin_info: CoinInfo,
    #[serde(rename = "RAW")]
    raw: Option<Raw>,
}

#[derive(Deserialize, Debug)]
struct Response {
    #[serde(rename = "Data")]
    data: Vec<Record>,
}

pub async fn query_cryptocompare(
    client: &Client,
    api_key: &SecretString,
) -> Result<HashMap<String, f64>, ()> {
    // ref: https://github.com/rust-lang/rust/pull/64856
    let header = format!("Apikey {}", api_key.expose_secret());
    let res = client
        .get(CRYPTOCOMPARE_URL.clone())
        // TODO don't copy the api key on every request
        .header("Authorization", header)
        .send()
        .map_err(|err| {
            error!(
                "Error fetching exchange rates from CryptoCompare: {:?}",
                err
            );
        })
        .await?;

    let res = res.error_for_status().map_err(|err| {
        error!(
            "HTTP error getting exchange rates from CryptoCompare: {:?}",
            err
        );
    })?;

    let res: Response = res
        .json()
        .map_err(|err| {
            error!(
            "Error getting exchange rate response body from CryptoCompare, incorrect type: {:?}",
            err
        );
        })
        .await?;

    let rates = res
        .data
        .into_iter()
        .filter_map(|asset| {
            if let Some(raw) = asset.raw {
                Some((asset.coin_info.name.to_uppercase(), raw.usd.price))
            } else {
                None
            }
        })
        .chain(once(("USD".to_string(), 1.0)));
    Ok(HashMap::from_iter(rates))
}
