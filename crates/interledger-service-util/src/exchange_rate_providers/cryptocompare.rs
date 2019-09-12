use futures::Future;
use lazy_static::lazy_static;
use log::error;
use reqwest::{r#async::Client, Url};
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
    raw: Raw,
}

#[derive(Deserialize, Debug)]
struct Response {
    #[serde(rename = "Data")]
    data: Vec<Record>,
}

pub fn query_cryptocompare(
    client: &Client,
    api_key: &SecretString,
) -> impl Future<Item = HashMap<String, f64>, Error = ()> {
    client
        .get(CRYPTOCOMPARE_URL.clone())
        // TODO don't copy the api key on every request
        .header(
            "Authorization",
            format!("Apikey {}", api_key.expose_secret()).as_str(),
        )
        .send()
        .map_err(|err| {
            error!("Error fetching exchange rates from CryptoCompare: {:?}", err);
        })
        .and_then(|res| {
            res.error_for_status().map_err(|err| {
                error!("HTTP error getting exchange rates from CryptoCompare: {:?}", err);
            })
        })
        .and_then(|mut res| {
            res.json().map_err(|err| {
                error!(
                    "Error getting exchange rate response body from CryptoCompare, incorrect type: {:?}",
                    err
                );
            })
        })
        .and_then(|res: Response| {
            let rates = res
                .data
                .into_iter()
                .map(|asset| (asset.coin_info.name.to_uppercase(), asset.raw.usd.price))
                .chain(once(("USD".to_string(), 1.0)));
            Ok(HashMap::from_iter(rates))
        })
}
