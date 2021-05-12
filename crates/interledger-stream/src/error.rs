use interledger_packet::{AddressError, PacketTypeError as IlpPacketTypeError};
/// Stream Errors
use std::str::Utf8Error;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    //TODO: can we remove these strings?
    #[error("Error polling: {0}")]
    SendMoneyError(String),
    #[error("Error maximum time exceeded: {0}")]
    TimeoutError(String),
}

#[derive(Debug, thiserror::Error)]
pub enum StreamPacketError {
    #[error("Unable to decrypt message")]
    FailedToDecrypt,
    #[error("Stream version not supported")]
    UnsupportedVersion,
    #[error("Frames Error: Not enough successfully parsed frames")]
    NotEnoughValidFrames,
    #[error("Trailing bytes error: Inner")]
    TrailingInnerBytes,
    #[error("Roundtrip only: Error expected for roundtrip fuzzing")]
    RoundtripError,
    #[error("I/O Error: {0}")]
    IoErr(#[from] std::io::Error),
    #[error("Ilp PacketType Error: {0}")]
    IplPacketTypeError(#[from] IlpPacketTypeError),
    #[error("Address Error: {0}")]
    AddressError(#[from] AddressError),
    #[error("UTF-8 Error: {0}")]
    Utf8Err(#[from] Utf8Error),
}
