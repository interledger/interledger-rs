use super::AddressError;

#[derive(Debug, thiserror::Error)]
pub enum ParseError {
    #[error("Invalid Packet: {0}")]
    Oer(#[from] OerError),
    #[error("Chrono Error: {0}")]
    ChronoErr(#[from] chrono::ParseError),
    #[error("Invalid Packet: {0}")]
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
    #[error("Unknown packet type")]
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

/// Object Encoding Rules errors happen with any low level representation reading.
///
/// See the [RFC-0030] for details.
/// [RFC-0030]: https://github.com/interledger/rfcs/blob/master/0030-notes-on-oer-encoding/0030-notes-on-oer-encoding.md
#[derive(PartialEq, Debug, thiserror::Error)]
pub enum OerError {
    #[error("buffer too small")]
    UnexpectedEof,
    #[error("{0}")]
    LengthPrefix(#[from] LengthPrefixError),
    #[error("{0}")]
    VarUint(#[from] VarUintError),
    #[error("{0}")]
    VariableLengthTimestamp(#[from] VariableLengthTimestampError),
}

#[derive(PartialEq, Debug, thiserror::Error)]
pub enum LengthPrefixError {
    #[error("indefinite lengths are not allowed")]
    IndefiniteLength,
    #[error("length prefix too large")]
    TooLarge,
    #[error("length prefix overflow")]
    UsizeOverflow,
    #[error("variable length prefix with unnecessary multibyte length")]
    LeadingZeros,
    #[cfg(feature = "strict")]
    #[cfg_attr(feature = "strict", error("length prefix with leading zero"))]
    StrictLeadingZeros,
}

#[derive(PartialEq, Debug, thiserror::Error)]
pub enum VarUintError {
    #[error("var uint has zero length")]
    ZeroLength,
    #[error("var uint too large")]
    TooLarge,
}

#[derive(PartialEq, Debug, thiserror::Error)]
pub enum VariableLengthTimestampError {
    #[error("Invalid length for variable length timestamp: {0}")]
    InvalidLength(usize),
    #[error("Input failed to parse as timestamp")]
    InvalidTimestamp,
}
