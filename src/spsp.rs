use bytes::Bytes;
use futures::{Future, Sink};
use hyper::service::service_fn_ok;
use hyper::{Body, Request, Response, Server, StatusCode};
use plugin::Plugin;
use reqwest::async::Client;
use ring::rand::{SecureRandom, SystemRandom};
use serde_json;
use std::sync::Arc;
use stream::{connect_async as connect_stream, Connection, StreamListener};
use tokio;

#[derive(Debug, Deserialize, Serialize)]
pub struct SpspResponse {
  destination_account: String,
  #[serde(with = "serde_base64")]
  shared_secret: Vec<u8>,
}

// From https://github.com/serde-rs/json/issues/360#issuecomment-330095360
mod serde_base64 {
  use base64;
  use serde::{de, Deserialize, Deserializer, Serializer};

  pub fn serialize<S>(bytes: &[u8], serializer: S) -> Result<S::Ok, S::Error>
  where
    S: Serializer,
  {
    serializer.serialize_str(&base64::encode(bytes))
  }

  pub fn deserialize<'de, D>(deserializer: D) -> Result<Vec<u8>, D::Error>
  where
    D: Deserializer<'de>,
  {
    let s = <&str>::deserialize(deserializer)?;
    // TODO also accept non-URL safe
    base64::decode(s).map_err(de::Error::custom)
  }
}

pub fn query(server: &str) -> impl Future<Item = SpspResponse, Error = ()> {
  Client::new()
    .get(server)
    .header("Accept", "application/spsp4+json")
    .send()
    .map_err(|err| {
      error!("Error querying SPSP server {:?}", err);
    }).and_then(|mut res| {
      debug!("Got SPSP response {:?}", res);
      res.json::<SpspResponse>().map_err(|err| {
        error!("Error parsing SPSP response: {:?}", err);
      })
    })
}

pub fn connect_async<S>(plugin: S, server: &str) -> impl Future<Item = Connection, Error = ()>
where
  S: Plugin + 'static,
{
  query(server)
    .and_then(|spsp| connect_stream(plugin, spsp.destination_account, spsp.shared_secret))
}

pub fn pay<S>(plugin: S, server: &str, source_amount: u64) -> impl Future<Item = u64, Error = ()>
where
  S: Plugin + 'static,
{
  connect_async(plugin, server)
    .and_then(move |conn: Connection| {
      let stream = conn.create_stream();
      stream.money.clone().send(source_amount)
        .and_then(move |_| {
          Ok(stream.money.total_delivered())
        })
    })
}

pub fn listen<S>(
  plugin: S,
  server_secret: Bytes,
  port: u16,
) -> impl Future<Item = StreamListener, Error = ()>
// TODO don't require it to be static
where
  S: Plugin + 'static,
{
  StreamListener::bind::<'static>(plugin, server_secret).and_then(
    move |(listener, connection_generator)| {
      let addr = ([127, 0, 0, 1], port).into();

      let secret_generator = Arc::new(connection_generator);
      let service = move || {
        let secret_generator = Arc::clone(&secret_generator);
        service_fn_ok(move |_req: Request<Body>| {
          let (destination_account, shared_secret) =
            secret_generator.generate_address_and_secret("");
          debug!(
            "Generated address and shared secret for account {}",
            destination_account
          );
          let spsp_response = SpspResponse {
            destination_account: destination_account.to_string(),
            shared_secret: shared_secret.to_vec(),
          };

          Response::builder()
            .header("Content-Type", "application/spsp4+json")
            .status(StatusCode::OK)
            .body(Body::from(serde_json::to_string(&spsp_response).unwrap()))
            .unwrap()
        })
      };

      // TODO give the user a way to turn it off
      let run_server = Server::bind(&addr).serve(service).map_err(|err| {
        error!("Server error: {:?}", err);
      });
      tokio::spawn(run_server);

      Ok(listener)
    },
  )
}

pub fn listen_with_random_secret<S>(
  plugin: S,
  port: u16,
) -> impl Future<Item = StreamListener, Error = ()>
// TODO don't require it to be static
where
  S: Plugin + 'static,
{
  let server_secret = random_secret();
  listen(plugin, server_secret, port)
}

fn random_secret() -> Bytes {
  let mut secret: [u8; 32] = [0; 32];
  SystemRandom::new().fill(&mut secret).unwrap();
  Bytes::from(&secret[..])
}
