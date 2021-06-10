use interledger_packet::OerError;
use std::str::Utf8Error;

#[derive(Debug, thiserror::Error)]
pub enum BtpPacketError {
    #[error("extra trailing bytes")]
    TrailingBytesErr,
    #[error("UTF-8 Error: {0}")]
    Utf8Err(#[from] Utf8Error),
    #[error("Chrono Error: {0}")]
    ChronoErr(#[from] chrono::ParseError),
    #[error("Invalid Packet: {0}")]
    PacketType(#[from] PacketTypeError),
    #[error("Invalid Packet: {0}")]
    Oer(#[from] OerError),
}

#[derive(Debug, thiserror::Error)]
pub enum PacketTypeError {
    #[error("PacketType {0} is not supported")]
    Unknown(u8),
    #[error("Cannot parse Message from packet of type {0}, expected type {1}")]
    Unexpected(u8, u8),
}
