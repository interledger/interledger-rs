use futures::Future;
use bytes::Bytes;
use reqwest::async::Client;
use plugin::Plugin;
use stream::{connect_async as connect_stream, Connection};
use std::sync::Arc;
use serde::{de, Deserialize, Deserializer};
use base64;

#[derive(Debug, Deserialize)]
pub struct SpspResponse {
  destination_account: String,
  #[serde(deserialize_with = "deserialize_base64")]
  shared_secret: Vec<u8>,
}

fn deserialize_base64<'de, D>(deserializer: D) -> Result<Vec<u8>, D::Error>
    where D: Deserializer<'de>
{
    let s = <&str>::deserialize(deserializer)?;
    base64::decode(s).map_err(de::Error::custom)
}

pub fn query(server: &str) -> impl Future<Item = SpspResponse, Error = ()> {
  Client::new()
    .get(server)
    .header("Accept", "application/spsp4+json")
    .send()
    .map_err(|err| {
      error!("Error querying SPSP server {:?}", err);
    })
    .and_then(|mut res| {
      res.json::<SpspResponse>()
        .map_err(|err| {
          error!("Error parsing SPSP response: {:?}", err);
        })
    })
}

pub fn connect_async<S>(plugin: S, server: &str) -> impl Future<Item = Arc<Connection>, Error = ()>
where
  S: Plugin + 'static
{
  query(server)
    .and_then(|spsp| {
      connect_stream(plugin, spsp.destination_account, spsp.shared_secret)
    })

}
