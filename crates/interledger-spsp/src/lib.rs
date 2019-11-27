//! # interledger-spsp
//!
//! Client and server implementations of the [Simple Payment Setup Protocol (SPSP)](https://github.com/interledger/rfcs/blob/master/0009-simple-payment-setup-protocol/0009-simple-payment-setup-protocol.md).
//!
//! This uses a simple HTTPS request to establish a shared key between the sender and receiver that is used to
//! authenticate ILP packets sent between them. SPSP uses the STREAM transport protocol for sending money and data over ILP.

use failure::Fail;
use interledger_packet::Address;
use interledger_stream::Error as StreamError;
use serde::{Deserialize, Serialize};

mod client;
mod server;

pub use client::{pay, query};
pub use server::SpspResponder;

// TODO should these error variants be renamed to remove the 'Error' suffix from each one?
#[derive(Fail, Debug)]
pub enum Error {
    #[fail(display = "Unable to query SPSP server: {:?}", _0)]
    HttpError(String),
    #[fail(display = "Got invalid SPSP response from server: {:?}", _0)]
    InvalidSpspServerResponseError(String),
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
    destination_account: Address,
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
