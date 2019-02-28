use futures::Future;
use interledger_service::{AccountId, IncomingService};
use interledger_stream::{send_money, Error as StreamError};
use reqwest::r#async::Client;

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

pub fn pay<S>(
    service: S,
    from_account: AccountId,
    receiver: &str,
    source_amount: u64,
) -> impl Future<Item = u64, Error = Error>
where
    S: IncomingService + Clone,
{
    query(receiver).and_then(move |spsp| {
        send_money(
            service,
            from_account,
            spsp.destination_account.as_bytes(),
            &spsp.shared_secret,
            source_amount,
        )
        .map(|(amount_delivered, _plugin)| amount_delivered)
        .map_err(move |_| Error::SendMoneyError(source_amount))
    })
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
