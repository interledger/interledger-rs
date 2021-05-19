use super::AddressError;

#[derive(Debug, thiserror::Error)]
pub enum ParseError {
    #[error("I/O Error: {0}")]
    IoErr(#[from] std::io::Error),
    #[error("Chrono Error: {0}")]
    ChronoErr(#[from] chrono::ParseError),
    #[error("PacketType Error: {0}")]
    PacketType(#[from] PacketTypeError),
    #[error("Invalid Packet: Reject.ErrorCode was not IA5String")]
    ErrorCodeConversion,
    #[error("Invalid Packet: DateTime must be numeric")]
    TimestampConversion,
    #[error("Invalid Address: {0}")]
    InvalidAddress(#[from] AddressError),
    #[error("Invalid Packet: {0}")]
    TrailingBytes(#[from] TrailingBytesError),
    #[cfg(feature = "roundtrip-only")]
    #[cfg_attr(feature = "roundtrip-only", error("Timestamp not roundtrippable"))]
    NonRoundtrippableTimestamp,
}

#[derive(Debug, thiserror::Error)]
pub enum PacketTypeError {
    #[error("Invalid Packet: Unknown packet type")]
    Eof,
    #[error("PacketType {0} is not supported")]
    Unknown(u8),
    #[error("PacketType {1} expected, found {0}")]
    Unexpected(u8, u8),
}

#[derive(Debug, thiserror::Error)]
pub enum TrailingBytesError {
    #[error("Unexpected outer trailing bytes")]
    Outer,
    #[error("Unexpected inner trailing bytes")]
    Inner,
}
