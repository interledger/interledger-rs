use interledger_packet::{
    AddressError, ErrorCode, OerError, PacketTypeError as IlpPacketTypeError,
};
/// Stream Errors
use std::str::Utf8Error;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("Terminating payment since too many packets are rejected ({0} packets fulfilled, {1} packets rejected)")]
    PaymentFailFast(u64, u64),
    #[error("Packet was rejected with ErrorCode: {0} {1:?}")]
    UnexpectedRejection(ErrorCode, String),
    #[error(
        "Error maximum time exceeded: Time since last fulfill exceeded the maximum time limit"
    )]
    Timeout,
}

#[derive(Debug, thiserror::Error)]
pub enum StreamPacketError {
    #[error("Unable to decrypt packet")]
    FailedToDecrypt,
    #[error("Unsupported STREAM version: {0}")]
    UnsupportedVersion(u8),
    #[error("Invalid Packet: Incorrect number of frames or unable to parse all frames")]
    NotEnoughValidFrames,
    #[error("Trailing bytes error: Inner")]
    TrailingInnerBytes,
    #[error("Invalid Packet: {0}")]
    Oer(#[from] OerError),
    #[error("Ilp PacketType Error: {0}")]
    IlpPacketType(#[from] IlpPacketTypeError),
    #[error("Address Error: {0}")]
    Address(#[from] AddressError),
    #[error("UTF-8 Error: {0}")]
    Utf8Err(#[from] Utf8Error),
    #[cfg(feature = "roundtrip-only")]
    #[cfg_attr(
        feature = "roundtrip-only",
        error("Roundtrip only: Error expected for roundtrip fuzzing")
    )]
    NonRoundtrippableSaturatingAmount,
}
