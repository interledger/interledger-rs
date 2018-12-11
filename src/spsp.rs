use bytes::Bytes;
use futures::Future;
use plugin::Plugin;
use reqwest::async::Client;
use ring::rand::{SecureRandom, SystemRandom};
use stream::oneshot::send_money;
use stream::{
    connect_async as connect_stream, Connection, ConnectionGenerator, Error as StreamError,
    StreamListener,
};
use tokio;
use tokio::net::TcpListener;
use tower_web::extract::{Context, Extract, Immediate};
use tower_web::util::BufStream;
use tower_web::ServiceBuilder;

#[derive(Fail, Debug)]
pub enum Error {
    #[fail(display = "Unable to query SPSP server: {:?}", _0)]
    HttpError(String),
    #[fail(display = "Got invalid SPSP response from server: {:?}", _0)]
    InvalidResponseError(String),
    #[fail(display = "STREAM error: {}", _0)]
    StreamError(StreamError),
    #[fail(display = "Error sending money: {}", _0)]
    SendMoneyError(u64),
    #[fail(display = "Error listening: {}", _0)]
    ListenError(String),
    #[fail(display = "Invalid Payment Pointer: {}", _0)]
    InvalidPaymentPointerError(String),
}

#[derive(Debug, Deserialize, Serialize, Response)]
#[web(header(name = "content-type", value = "application/spsp4+json"))]
#[web(header(name = "access-control-allow-origin", value = "*"))]
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

pub fn query(server: &str) -> impl Future<Item = SpspResponse, Error = Error> {
    let server = payment_pointer_to_url(server);

    let client = Client::new();
    client
        .get(&server)
        .header("Accept", "application/spsp4+json")
        .send()
        .map_err(|err| Error::HttpError(format!("{:?}", err)))
        .and_then(|mut res| {
            res.json::<SpspResponse>()
                .map_err(|err| Error::InvalidResponseError(format!("{:?}", err)))
        })
}

pub fn connect_async<S>(plugin: S, server: &str) -> impl Future<Item = Connection, Error = Error>
where
    S: Plugin + 'static,
{
    query(server).and_then(|spsp| {
        connect_stream(plugin, spsp.destination_account, spsp.shared_secret.clone())
            .map_err(Error::StreamError)
    })
}

pub fn pay<S>(plugin: S, server: &str, source_amount: u64) -> impl Future<Item = u64, Error = Error>
where
    S: Plugin + 'static,
{
    query(server).and_then(move |spsp| {
        send_money(
            plugin,
            spsp.destination_account,
            spsp.shared_secret,
            source_amount,
        )
        .map(|(amount_delivered, _plugin)| amount_delivered)
        .map_err(move |_| Error::SendMoneyError(source_amount))
    })
}

pub struct OriginalUrl(String);

impl<B: BufStream> Extract<B> for OriginalUrl {
    type Future = Immediate<Self>;

    fn extract(ctx: &Context) -> Self::Future {
        let headers = ctx.request().headers();
        let host = headers
            .get("forwarded")
            .and_then(|header| {
                let header = header.to_str().ok()?;
                if let Some(index) = header.find(" for=") {
                    let host_start = index + 5;
                    (&header[host_start..]).split_whitespace().next()
                } else {
                    None
                }
            })
            .or_else(|| {
                headers
                    .get("x-forwarded-host")
                    .and_then(|header| header.to_str().ok())
            })
            .or_else(|| headers.get("host").and_then(|header| header.to_str().ok()))
            .unwrap_or("");

        let mut url = host.to_string();
        url.push_str(ctx.request().uri().path());
        url.push_str(ctx.request().uri().query().unwrap_or(""));
        Immediate::ok(OriginalUrl(url))
    }
}

pub struct SpspResponder {
    connection_generator: ConnectionGenerator,
}

impl_web! {
    impl SpspResponder {
        fn get_spsp(&self, url: &str) -> Result<SpspResponse, ()> {
            let (destination_account, shared_secret) = self.connection_generator.generate_address_and_secret(url);
                    debug!(
                        "Responding to SPSP query {} with address: {}",
                        url, destination_account
                    );
            Ok(SpspResponse {
                destination_account,
                shared_secret: shared_secret.to_vec(),
            })
        }

        #[get("/.well-known/pay")]
        #[content_type("json")]
        fn get_well_known(&self, original_url: OriginalUrl) -> Result<SpspResponse, ()> {
            self.get_spsp(&original_url.0)
        }

        #[get("/spsp/:user")]
        #[content_type("json")]
        fn get_spsp_user(&self, original_url: OriginalUrl) -> Result<SpspResponse, ()> {
            self.get_spsp(&original_url.0)
        }

        // TODO should we also allow http://domain.example/user ?
    }
}

pub fn listen<S>(
    plugin: S,
    server_secret: Bytes,
    port: u16,
) -> impl Future<Item = StreamListener, Error = Error>
// TODO don't require it to be static
where
    S: Plugin + 'static,
{
    StreamListener::bind::<'static>(plugin, server_secret)
        .map_err(Error::StreamError)
        .and_then(move |(listener, connection_generator)| {
            let addr = ([127, 0, 0, 1], port).into();
            let tcp_listener =
                TcpListener::bind(&addr).map_err(|err| Error::ListenError(format!("{:?}", err)))?;

            let server = ServiceBuilder::new()
                .resource(SpspResponder {
                    connection_generator,
                })
                .serve(tcp_listener.incoming());
            tokio::spawn(server);

            Ok(listener)
        })
}

pub fn listen_with_random_secret<S>(
    plugin: S,
    port: u16,
) -> impl Future<Item = StreamListener, Error = Error>
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

fn payment_pointer_to_url(payment_pointer: &str) -> String {
    let mut url: String = if payment_pointer.starts_with('$') {
        let mut url = "https://".to_string();
        url.push_str(&payment_pointer[1..]);
        url
    } else {
        payment_pointer.to_string()
    };

    let num_slashes = url.matches('/').count();
    if num_slashes == 0 {
        url.push_str("/.well-known/pay");
    } else if num_slashes == 1 && url.ends_with('/') {
        url.push_str(".well-known/pay");
    }
    url
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn get_details_well_known_endpoint() {
        let server = SpspResponder {
            connection_generator: ConnectionGenerator::new("example.server", &[0; 32][..]),
        };
        server
            .get_well_known(OriginalUrl("domain.example/.well-known/pay".to_string()))
            .unwrap();
    }

    #[test]
    fn get_details_spsp_user() {
        let server = SpspResponder {
            connection_generator: ConnectionGenerator::new("example.server", &[0; 32][..]),
        };
        server
            .get_well_known(OriginalUrl("domain.example/spsp/bob".to_string()))
            .unwrap();
    }
}
