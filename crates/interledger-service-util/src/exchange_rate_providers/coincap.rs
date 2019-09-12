use futures::Future;
use lazy_static::lazy_static;
use log::{error, warn};
use reqwest::{r#async::Client, Url};
use serde::Deserialize;
use std::{collections::HashMap, str::FromStr};

lazy_static! {
    // We use both endpoints because they contain different sets of rates
    // This one has more cryptocurrencies
    static ref COINCAP_ASSETS_URL: Url = Url::parse("https://api.coincap.io/v2/assets").unwrap();
    // This one has more fiat currencies
    static ref COINCAP_RATES_URL: Url = Url::parse("https://api.coincap.io/v2/rates").unwrap();
}

#[derive(Deserialize, Debug)]
struct Rate {
    symbol: String,
    #[serde(alias = "rateUsd", alias = "priceUsd")]
    rate_usd: String,
}

#[derive(Deserialize, Debug)]
struct RateResponse {
    data: Vec<Rate>,
}

pub fn query_coincap(client: &Client) -> impl Future<Item = HashMap<String, f64>, Error = ()> {
    query_coincap_endpoint(client, COINCAP_ASSETS_URL.clone())
        .join(query_coincap_endpoint(client, COINCAP_RATES_URL.clone()))
        .and_then(|(assets, rates)| {
            let all_rates: HashMap<String, f64> = assets
                .data
                .into_iter()
                .chain(rates.data.into_iter())
                .filter_map(|record| match f64::from_str(record.rate_usd.as_str()) {
                    Ok(rate) => Some((record.symbol.to_uppercase(), rate)),
                    Err(err) => {
                        warn!(
                            "Unable to parse {} rate as an f64: {} {:?}",
                            record.symbol, record.rate_usd, err
                        );
                        None
                    }
                })
                .collect();
            Ok(all_rates)
        })
}

fn query_coincap_endpoint(
    client: &Client,
    url: Url,
) -> impl Future<Item = RateResponse, Error = ()> {
    client
        .get(url)
        .send()
        .map_err(|err| {
            error!("Error fetching exchange rates from CoinCap: {:?}", err);
        })
        .and_then(|res| {
            res.error_for_status().map_err(|err| {
                error!("HTTP error getting exchange rates from CoinCap: {:?}", err);
            })
        })
        .and_then(|mut res| {
            res.json().map_err(|err| {
                error!(
                    "Error getting exchange rate response body from CoinCap, incorrect type: {:?}",
                    err
                );
            })
        })
}
