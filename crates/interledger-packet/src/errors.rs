use super::AddressError;
use std::str::Utf8Error;
use std::string::FromUtf8Error;

#[derive(Debug, thiserror::Error)]
pub enum ParseError {
    #[error("I/O Error: {0}")]
    IoErr(#[from] std::io::Error),
    #[error("Chrono Error: {0}")]
    ChronoErr(#[from] chrono::ParseError),
    #[error("PacketType Error: {0}")]
    PacketTypeError(#[from] PacketTypeError),
    #[error("Trailing Bytes Error: {0}")]
    TrailingBytesError(#[from] TrailingBytesError),
    // TODO: use specific errors for timestamp etc
    #[error("Data Type Error: {0}")]
    DataTypeError(#[from] DataTypeError),
    #[error("Wrong Type: {0}")]
    WrongType(String),
    #[error("Invalid Address: {0}")]
    InvalidAddress(#[from] AddressError),
    #[error("Invalid Packet: {0}")]
    InvalidPacket(String),
    #[error("Roundtrip(Fuzzing) Error")]
    RoundtripError,
    // TODO: move below to other crates
    #[error("UTF-8 Error: {0}")]
    Utf8Err(#[from] Utf8Error),
    #[error("UTF-8 Conversion Error: {0}")]
    FromUtf8Err(#[from] FromUtf8Error),
}

#[derive(Debug, thiserror::Error)]
pub enum PacketTypeError {
    #[error("PacketType data not found")]
    EOF,
    #[error("PacketType {0} is not supported")]
    Unknown(u8),
    #[error("PacketType {1} expected, found {0}")]
    Unexpected(u8, u8),
}

#[derive(Debug, thiserror::Error)]
pub enum TrailingBytesError {
    #[error("Outer")]
    Outer,
    #[error("Inner")]
    Inner,
}

#[derive(Debug, thiserror::Error)]
pub enum DataTypeError {
    #[error("Should be IA5String")]
    IA5String,
    #[error("Should be ASCII")]
    ASCII,
}
