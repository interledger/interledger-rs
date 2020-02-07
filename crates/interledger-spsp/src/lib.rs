//! # interledger-spsp
//!
//! Client and server implementations of the [Simple Payment Setup Protocol (SPSP)](https://github.com/interledger/rfcs/blob/master/0009-simple-payment-setup-protocol/0009-simple-payment-setup-protocol.md).
//!
//! This uses a simple HTTPS request to establish a shared key between the sender and receiver that is used to
//! authenticate ILP packets sent between them. SPSP uses the STREAM transport protocol for sending money and data over ILP.

use interledger_packet::Address;
use interledger_stream::Error as StreamError;
use serde::{Deserialize, Serialize};

/// An SPSP client which can query an SPSP Server's payment pointer and initiate a STREAM payment
mod client;
/// An SPSP Server implementing an HTTP Service which generates ILP Addresses and Shared Secrets
mod server;

pub use client::{pay, query};
pub use server::SpspResponder;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("Unable to query SPSP server: {0}")]
    HttpError(String),
    #[error("Got invalid SPSP response from server: {0}")]
    InvalidSpspServerResponseError(String),
    #[error("STREAM error: {0}")]
    StreamError(#[from] StreamError),
    #[error("Error sending money: {0}")]
    SendMoneyError(u64),
    #[error("Error listening: {0}")]
    ListenError(String),
    #[error("Invalid Payment Pointer: {0}")]
    InvalidPaymentPointerError(String),
}

/// An SPSP Response returned by the SPSP server
#[derive(Debug, Deserialize, Serialize)]
pub struct SpspResponse {
    /// The generated ILP Address for this SPSP connection
    destination_account: Address,
    /// Base-64 encoded shared secret between SPSP client and server
    /// to be consumed for the STREAM connection
    #[serde(with = "serde_base64")]
    shared_secret: Vec<u8>,
}

// From https://github.com/serde-rs/json/issues/360#issuecomment-330095360
#[doc(hidden)]
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
