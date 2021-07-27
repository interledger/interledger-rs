use std::fmt;
use std::str;
use std::time::SystemTime;

use bytes::{Buf, BufMut, BytesMut};
use chrono::{DateTime, TimeZone, Utc};

use crate::oer::{self, BufOerExt, MutBufOerExt};
use crate::{hex::HexString, OerError};
use crate::{Address, ErrorCode, PacketTypeError, ParseError, TrailingBytesError};
use std::convert::TryFrom;
use std::io::Write;

const AMOUNT_LEN: usize = 8;
const EXPIRY_LEN: usize = 17;
const CONDITION_LEN: usize = 32;
const FULFILLMENT_LEN: usize = 32;
const ERROR_CODE_LEN: usize = 3;

// NOTE: this is strictly different from the oer::GENERALIZED_TIMESTAMP_FORMAT which has a dot, and
// is used for much more lenient timestamps with 0-3 fractions.
static INTERLEDGER_TIMESTAMP_FORMAT: &str = "%Y%m%d%H%M%S%3f";

#[derive(Clone, Copy, Debug, PartialEq)]
#[repr(u8)]
pub enum PacketType {
    Prepare = 12,
    Fulfill = 13,
    Reject = 14,
}

impl PacketType {
    /// Length of the packet type on the wire
    pub const LEN: usize = 1;
}

// Gets the packet type from a u8 array
impl TryFrom<&[u8]> for PacketType {
    type Error = PacketTypeError;

    fn try_from(bytes: &[u8]) -> Result<Self, Self::Error> {
        let first = bytes.first().ok_or(PacketTypeError::Eof)?;

        PacketType::try_from(*first)
    }
}

impl TryFrom<u8> for PacketType {
    type Error = PacketTypeError;

    fn try_from(byte: u8) -> Result<Self, Self::Error> {
        match byte {
            12 => Ok(PacketType::Prepare),
            13 => Ok(PacketType::Fulfill),
            14 => Ok(PacketType::Reject),
            byte => Err(PacketTypeError::Unknown(byte)),
        }
    }
}

#[derive(Debug, PartialEq, Clone)]
pub enum Packet {
    Prepare(Prepare),
    Fulfill(Fulfill),
    Reject(Reject),
}

impl TryFrom<BytesMut> for Packet {
    type Error = ParseError;

    fn try_from(buffer: BytesMut) -> Result<Self, Self::Error> {
        match PacketType::try_from(buffer.as_ref())? {
            PacketType::Prepare => Prepare::try_from(buffer).map(Packet::from),
            PacketType::Fulfill => Fulfill::try_from(buffer).map(Packet::from),
            PacketType::Reject => Reject::try_from(buffer).map(Packet::from),
        }
    }
}

impl From<Packet> for BytesMut {
    fn from(packet: Packet) -> Self {
        match packet {
            Packet::Prepare(prepare) => prepare.into(),
            Packet::Fulfill(fulfill) => fulfill.into(),
            Packet::Reject(reject) => reject.into(),
        }
    }
}

impl From<Prepare> for Packet {
    fn from(prepare: Prepare) -> Self {
        Packet::Prepare(prepare)
    }
}

impl From<Fulfill> for Packet {
    fn from(fulfill: Fulfill) -> Self {
        Packet::Fulfill(fulfill)
    }
}

impl From<Reject> for Packet {
    fn from(reject: Reject) -> Self {
        Packet::Reject(reject)
    }
}

#[derive(PartialEq, Clone)]
pub struct Prepare {
    buffer: BytesMut,
    content_offset: usize,
    destination: Address,
    amount: u64,
    expires_at: SystemTime,
    data_offset: usize,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PrepareBuilder<'a> {
    pub amount: u64,
    pub expires_at: SystemTime,
    pub execution_condition: &'a [u8; 32],
    pub destination: Address,
    pub data: &'a [u8],
}

impl TryFrom<BytesMut> for Prepare {
    type Error = ParseError;

    fn try_from(buffer: BytesMut) -> Result<Self, Self::Error> {
        let (content_offset, mut content) = deserialize_envelope(PacketType::Prepare, &buffer)?;
        let content_len = content.len();

        const MIN_LEN: usize = AMOUNT_LEN
            + EXPIRY_LEN
            + CONDITION_LEN
            // destination
            + Address::MIN_LEN
            // data
            + oer::EMPTY_VARLEN_OCTETS_LEN;

        if content.remaining() < MIN_LEN {
            return Err(OerError::UnexpectedEof.into());
        }

        let amount = content.get_u64();

        // Fixed Length DateTime format - RFC 0027
        // https://github.com/interledger/rfcs/blob/2dfdcf47ac52489a4ad473a5d869cd9f0217db67/0027-interledger-protocol-4/0027-interledger-protocol-4.md#ilp-prepare
        let mut read_expires_at = [0x00; 17];
        content.copy_to_slice(&mut read_expires_at);

        if !read_expires_at
            .iter()
            .map(|e| (b'0'..=b'9').contains(e))
            .fold(true, |a, b| a & b)
        {
            return Err(ParseError::TimestampConversion);
        }

        let expires_at = str::from_utf8(&read_expires_at[..])
            .expect("read_expires_at matches only ascii, utf8 conversion must succeed");
        let expires_at: DateTime<Utc> =
            Utc.datetime_from_str(expires_at, INTERLEDGER_TIMESTAMP_FORMAT)?;
        let expires_at = SystemTime::from(expires_at);

        #[cfg(feature = "roundtrip-only")]
        {
            // chrono will leniently parse some timestamps into forms which don't roundtrip.
            // this works around the class of fuzzer findings demonstrated by
            // fuzzed_1_chrono_60s_rollover.
            let mut roundtripped = [0u8; 17];
            write!(
                &mut roundtripped[..],
                "{}",
                DateTime::<Utc>::from(expires_at).format(INTERLEDGER_TIMESTAMP_FORMAT),
            )
            .unwrap();

            if roundtripped != read_expires_at {
                return Err(ParseError::NonRoundtrippableTimestamp);
            }
        }

        // Skip execution condition.
        content.skip(CONDITION_LEN)?;

        let destination = Address::try_from(content.read_var_octet_string()?)?;

        // Skip the data.
        let data_offset = content_offset + content_len - content.len();
        content.skip_var_octet_string()?;

        ensure_no_inner_trailing_bytes(content)?;

        Ok(Prepare {
            buffer,
            content_offset,
            destination,
            amount,
            expires_at,
            data_offset,
        })
    }
}

impl Prepare {
    #[inline]
    pub fn amount(&self) -> u64 {
        self.amount
    }

    #[inline]
    pub fn set_amount(&mut self, amount: u64) {
        self.amount = amount;
        let mut buffer = &mut self.buffer[self.content_offset..self.content_offset + 8];
        buffer.put_u64(amount);
    }

    #[inline]
    pub fn expires_at(&self) -> SystemTime {
        self.expires_at
    }

    #[inline]
    pub fn set_expires_at(&mut self, expires_at: SystemTime) {
        self.expires_at = expires_at;
        let offset = self.content_offset + AMOUNT_LEN;
        write!(
            &mut self.buffer[offset..offset + EXPIRY_LEN],
            "{}",
            DateTime::<Utc>::from(expires_at).format(INTERLEDGER_TIMESTAMP_FORMAT),
        )
        .unwrap();
    }

    /// The returned value always has a length of 32.
    #[inline]
    pub fn execution_condition(&self) -> &[u8] {
        let begin = self.content_offset + AMOUNT_LEN + EXPIRY_LEN;
        let end = begin + CONDITION_LEN;
        &self.buffer[begin..end]
    }

    #[inline]
    pub fn destination(&self) -> Address {
        self.destination.clone()
    }

    #[inline]
    pub fn data(&self) -> &[u8] {
        (&self.buffer[self.data_offset..])
            .peek_var_octet_string()
            .unwrap()
    }

    #[inline]
    pub fn into_data(mut self) -> BytesMut {
        oer::extract_var_octet_string(self.buffer.split_off(self.data_offset)).unwrap()
    }
}

impl AsRef<[u8]> for Prepare {
    #[inline]
    fn as_ref(&self) -> &[u8] {
        &self.buffer
    }
}

impl fmt::Debug for Prepare {
    fn fmt(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        formatter
            .debug_struct("Prepare")
            .field("destination", &self.destination())
            .field("amount", &self.amount())
            .field(
                "expires_at",
                &DateTime::<Utc>::from(self.expires_at()).to_rfc3339(),
            )
            .field(
                "execution_condition",
                &HexString(self.execution_condition()),
            )
            .field("data_length", &self.data().len())
            .finish()
    }
}

impl<'a> PrepareBuilder<'a> {
    pub fn build(&self) -> Prepare {
        const STATIC_LEN: usize = AMOUNT_LEN + EXPIRY_LEN + CONDITION_LEN;
        let destination_size = oer::predict_var_octet_string(self.destination.len());
        let data_size = oer::predict_var_octet_string(self.data.len());
        let content_len = STATIC_LEN + destination_size + data_size;
        let buf_size = 1 + oer::predict_var_octet_string(content_len);
        let mut buffer = BytesMut::with_capacity(buf_size);

        buffer.put_u8(PacketType::Prepare as u8);
        buffer.put_var_octet_string_length(content_len);
        let content_offset = buffer.len();
        buffer.put_u64(self.amount);

        let mut writer = buffer.writer();
        write!(
            writer,
            "{}",
            DateTime::<Utc>::from(self.expires_at).format(INTERLEDGER_TIMESTAMP_FORMAT),
        )
        .unwrap();
        let mut buffer = writer.into_inner();

        buffer.put_slice(&self.execution_condition[..]);
        buffer.put_var_octet_string::<&[u8]>(self.destination.as_ref());
        buffer.put_var_octet_string(self.data);

        Prepare {
            buffer,
            content_offset,
            destination: self.destination.clone(),
            amount: self.amount,
            expires_at: self.expires_at,
            data_offset: buf_size - data_size,
        }
    }
}

#[derive(PartialEq, Clone)]
pub struct Fulfill {
    buffer: BytesMut,
    content_offset: usize,
}

#[derive(Clone, Debug, PartialEq)]
pub struct FulfillBuilder<'a> {
    pub fulfillment: &'a [u8; 32],
    pub data: &'a [u8],
}

impl TryFrom<BytesMut> for Fulfill {
    type Error = ParseError;

    fn try_from(buffer: BytesMut) -> Result<Self, Self::Error> {
        let (content_offset, mut content) = deserialize_envelope(PacketType::Fulfill, &buffer)?;

        content.skip(FULFILLMENT_LEN)?;
        content.skip_var_octet_string()?;

        ensure_no_inner_trailing_bytes(content)?;

        Ok(Fulfill {
            buffer,
            content_offset,
        })
    }
}

impl Fulfill {
    /// The returned value always has a length of 32.
    // FIXME: the return value could now be &[u8; 32]
    #[inline]
    pub fn fulfillment(&self) -> &[u8] {
        let begin = self.content_offset;
        let end = begin + FULFILLMENT_LEN;
        &self.buffer[begin..end]
    }

    #[inline]
    pub fn data(&self) -> &[u8] {
        let data_offset = self.content_offset + FULFILLMENT_LEN;
        (&self.buffer[data_offset..])
            .peek_var_octet_string()
            .unwrap()
    }

    #[inline]
    pub fn into_data(mut self) -> BytesMut {
        let data_offset = self.content_offset + FULFILLMENT_LEN;
        oer::extract_var_octet_string(self.buffer.split_off(data_offset)).unwrap()
    }
}

impl AsRef<[u8]> for Fulfill {
    #[inline]
    fn as_ref(&self) -> &[u8] {
        &self.buffer
    }
}

impl From<Fulfill> for BytesMut {
    fn from(fulfill: Fulfill) -> Self {
        fulfill.buffer
    }
}

impl fmt::Debug for Fulfill {
    fn fmt(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        formatter
            .debug_struct("Fulfill")
            .field("fulfillment", &HexString(self.fulfillment()))
            .field("data_length", &self.data().len())
            .finish()
    }
}

impl<'a> FulfillBuilder<'a> {
    pub fn build(&self) -> Fulfill {
        let data_size = oer::predict_var_octet_string(self.data.len());
        let content_len = FULFILLMENT_LEN + data_size;
        let buf_size = 1 + oer::predict_var_octet_string(content_len);
        let mut buffer = BytesMut::with_capacity(buf_size);

        buffer.put_u8(PacketType::Fulfill as u8);
        buffer.put_var_octet_string_length(content_len);
        let content_offset = buffer.len();
        buffer.put_slice(&self.fulfillment[..]);
        buffer.put_var_octet_string(self.data);
        Fulfill {
            buffer,
            content_offset,
        }
    }
}

#[derive(PartialEq, Clone)]
pub struct Reject {
    buffer: BytesMut,
    code: ErrorCode,
    message_offset: usize,
    triggered_by_offset: usize,
    data_offset: usize,
}

#[derive(Clone, Debug, PartialEq)]
pub struct RejectBuilder<'a> {
    pub code: ErrorCode,
    pub message: &'a [u8],
    pub triggered_by: Option<&'a Address>,
    pub data: &'a [u8],
}

impl TryFrom<BytesMut> for Reject {
    type Error = ParseError;

    fn try_from(buffer: BytesMut) -> Result<Self, Self::Error> {
        let (content_offset, mut content) = deserialize_envelope(PacketType::Reject, &buffer)?;
        let content_len = content.len();

        const MIN_LEN: usize = ERROR_CODE_LEN + oer::EMPTY_VARLEN_OCTETS_LEN * 3;

        if content.remaining() < MIN_LEN {
            return Err(OerError::UnexpectedEof.into());
        }

        let mut code = [0; 3];
        content.copy_to_slice(&mut code);

        let code = ErrorCode::new(code).ok_or(ParseError::ErrorCodeConversion)?;

        let triggered_by_offset = content_offset + content_len - content.len();
        Address::try_from(content.read_var_octet_string()?)?;

        let message_offset = content_offset + content_len - content.len();
        content.skip_var_octet_string()?;

        let data_offset = content_offset + content_len - content.len();
        content.skip_var_octet_string()?;

        ensure_no_inner_trailing_bytes(content)?;

        Ok(Reject {
            buffer,
            code,
            message_offset,
            triggered_by_offset,
            data_offset,
        })
    }
}

impl Reject {
    #[inline]
    pub fn code(&self) -> ErrorCode {
        self.code
    }

    #[inline]
    pub fn triggered_by(&self) -> Option<Address> {
        match (&self.buffer[self.triggered_by_offset..]).peek_var_octet_string() {
            Ok(bytes) => Address::try_from(bytes).ok(),
            Err(_) => None,
        }
    }

    #[inline]
    pub fn message(&self) -> &[u8] {
        (&self.buffer[self.message_offset..])
            .peek_var_octet_string()
            .unwrap()
    }

    #[inline]
    pub fn data(&self) -> &[u8] {
        (&self.buffer[self.data_offset..])
            .peek_var_octet_string()
            .unwrap()
    }

    pub fn into_data(mut self) -> BytesMut {
        oer::extract_var_octet_string(self.buffer.split_off(self.data_offset)).unwrap()
    }
}

impl AsRef<[u8]> for Reject {
    #[inline]
    fn as_ref(&self) -> &[u8] {
        &self.buffer
    }
}

impl From<Reject> for BytesMut {
    fn from(reject: Reject) -> Self {
        reject.buffer
    }
}

impl fmt::Debug for Reject {
    fn fmt(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        formatter
            .debug_struct("Reject")
            .field("code", &self.code())
            .field(
                "message",
                &str::from_utf8(self.message()).map_err(|_| fmt::Error)?,
            )
            .field("triggered_by", &self.triggered_by())
            .field("data_length", &self.data().len())
            .finish()
    }
}

impl<'a> RejectBuilder<'a> {
    pub fn build(&self) -> Reject {
        let (trigerred_by_message, len) = match self.triggered_by {
            Some(ref msg) => (msg.as_ref(), msg.len()),
            None => {
                let empty_msg: &[u8] = &[];
                (empty_msg, 0)
            }
        };
        let triggered_by_size = oer::predict_var_octet_string(len);
        let message_size = oer::predict_var_octet_string(self.message.len());
        let data_size = oer::predict_var_octet_string(self.data.len());
        let content_len = ERROR_CODE_LEN + triggered_by_size + message_size + data_size;
        let buf_size = 1 + oer::predict_var_octet_string(content_len);
        let mut buffer = BytesMut::with_capacity(buf_size);

        buffer.put_u8(PacketType::Reject as u8);
        buffer.put_var_octet_string_length(content_len);
        buffer.put_slice(&<[u8; 3]>::from(self.code)[..]);
        buffer.put_var_octet_string::<&[u8]>(trigerred_by_message);
        buffer.put_var_octet_string(self.message);
        buffer.put_var_octet_string(self.data);
        Reject {
            buffer,
            code: self.code,
            triggered_by_offset: buf_size - data_size - message_size - triggered_by_size,
            message_offset: buf_size - data_size - message_size,
            data_offset: buf_size - data_size,
        }
    }
}

/// Parses the outermost ILP packet ("envelope") to expected type (first byte).
///
/// ```text
/// +------+------------+---------//--+
/// | type |   varlen   |   content   |
/// |  u8  | 1..9 bytes |             |
/// +------+------------+---------//--+
///  0      1            n
///                      ^
///                      |
///  returned (offset and slice start)
/// ```
///
/// Returns the offset and slice starting with the offset.
fn deserialize_envelope(
    packet_type: PacketType,
    mut reader: &[u8],
) -> Result<(usize, &[u8]), ParseError> {
    if reader.remaining() < PacketType::LEN {
        return Err(OerError::UnexpectedEof.into());
    }
    let got_type = reader.get_u8();

    if got_type != packet_type as u8 {
        return Err(PacketTypeError::Unexpected(got_type, packet_type as u8).into());
    }

    let content_offset = PacketType::LEN + {
        // This could probably be determined a better way...
        let mut peek = reader;
        let before = peek.len();
        peek.read_var_octet_string_length()?;
        before - peek.len()
    };

    let content = reader.read_var_octet_string()?;

    ensure_no_outer_trailing_bytes(reader)?;

    Ok((content_offset, content))
}

#[derive(Clone, Debug, PartialEq)]
pub struct MaxPacketAmountDetails {
    amount_received: u64,
    max_amount: u64,
}

impl MaxPacketAmountDetails {
    #[inline]
    pub fn new(amount_received: u64, max_amount: u64) -> Self {
        MaxPacketAmountDetails {
            amount_received,
            max_amount,
        }
    }

    // Convert to use TryFrom? Also probably should go to max_packet_amount.rs
    pub fn from_bytes<B: Buf>(mut bytes: B) -> Result<Self, std::io::Error> {
        if bytes.remaining() < 16 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                // copied from read_exact
                "failed to fill whole buffer",
            ));
        }
        let amount_received = bytes.get_u64();
        let max_amount = bytes.get_u64();
        Ok(MaxPacketAmountDetails::new(amount_received, max_amount))
    }

    pub fn to_bytes(&self) -> [u8; 16] {
        let mut bytes = [0x00u8; 16];

        let buf = self.amount_received.to_be_bytes();
        bytes[..8].copy_from_slice(&buf[..]);

        let buf = self.max_amount.to_be_bytes();
        bytes[8..].copy_from_slice(&buf[..]);

        bytes
    }

    #[inline]
    pub fn amount_received(&self) -> u64 {
        self.amount_received
    }

    #[inline]
    pub fn max_amount(&self) -> u64 {
        self.max_amount
    }
}

impl From<Prepare> for BytesMut {
    fn from(prepare: Prepare) -> Self {
        prepare.buffer
    }
}

/// Called at the end of each parse to ensure that the given buffer is empty, as in there are no
/// trailing bytes. Calling these bytes as the "inner" bytes as opposed to "outer" bytes seen by
/// the [`deserialize_envelope`].
fn ensure_no_inner_trailing_bytes(content: &[u8]) -> Result<(), TrailingBytesError> {
    if content.is_empty() || !cfg!(feature = "strict") {
        Ok(())
    } else {
        Err(TrailingBytesError::Inner)
    }
}

/// Called at the end of [`deserialize_envelope`] to make sure that there are no trailing bytes
/// after the outermost variable length container which are called "outer".
fn ensure_no_outer_trailing_bytes(reader: &[u8]) -> Result<(), TrailingBytesError> {
    if reader.is_empty() || !cfg!(feature = "strict") {
        Ok(())
    } else {
        Err(TrailingBytesError::Outer)
    }
}

#[cfg(test)]
mod fuzzed {
    use super::Packet;
    use bytes::BytesMut;
    use std::convert::TryFrom;

    #[test]
    fn fuzzed_0_non_ascii_error_code() {
        #[rustfmt::skip]
        let orig = [
            // reject
            14,
            // varlen for the content
            13,
            // the invalid error code
            116, 119, 255,
            6, 116, 101, 115, 116, 46, 116, 0, 0, 42,
        ];

        let e = Packet::try_from(BytesMut::from(&orig[..])).unwrap_err();
        assert_eq!(
            "Invalid Packet: Reject.ErrorCode was not IA5String",
            &e.to_string(),
        );
    }

    #[test]
    fn fuzzed_1_with_fixed_errorcode() {
        #[rustfmt::skip]
        let orig = [
            // reject
            14,
            // varlen for the content
            13,
            // fixed error code
            116, 119, 127,
            // varlen address
            6, 116, 101, 115, 116, 46, 116,
            // varlen message
            0,
            // varlen data
            0,
            // extra byte in the end
            42,
        ];

        let res = Packet::try_from(BytesMut::from(&orig[..]));

        if cfg!(feature = "strict") {
            assert_eq!(
                "Invalid Packet: Unexpected inner trailing bytes",
                &res.unwrap_err().to_string(),
            );
        } else {
            res.unwrap();
        }
    }

    #[test]
    #[cfg(feature = "roundtrip-only")]
    fn fuzzed_2_chrono_60s_rollover() {
        // this has been reduced from the original
        #[rustfmt::skip]
        let orig = [
            // prepare
            12,
            // variable length len
            65,
            // amount
            0, 0, 0, 0, 0, 0, 0, 1,
            // timestamp, "2000-05-31T16:01:60.251" which is parsed and later formatted as
            // "2000-05-31T16:02:00.251" (dashes, T, colons and dot added for readability)
            50, 48, 48, 48, 48, 53, 51, 49, 49, 54, 48, 49, 54, 48, 50, 53, 49,
            // execution condition
            0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15,
            16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27, 28, 29, 30, 31,
            // destination varlen
            6,
            // destination "test.a"
            116, 101, 115, 116, 46, 97,
            // data varlen
            0,
        ];

        crate::lenient_packet_roundtrips(&orig[..]).unwrap_err();
    }
}

#[cfg(test)]
mod test_packet_type {
    use super::*;

    #[test]
    fn test_try_from() {
        assert_eq!(PacketType::try_from(12).unwrap(), PacketType::Prepare);
        assert_eq!(PacketType::try_from(13).unwrap(), PacketType::Fulfill);
        assert_eq!(PacketType::try_from(14).unwrap(), PacketType::Reject);
        assert!(PacketType::try_from(15).is_err());
    }

    #[test]
    fn try_from_empty() {
        assert_eq!(
            "Unknown packet type",
            &PacketType::try_from(&[][..]).unwrap_err().to_string()
        );
    }
}

#[cfg(test)]
mod test_packet {
    use super::*;
    use crate::fixtures::{FULFILL, PREPARE, REJECT};
    use crate::fixtures::{FULFILL_BYTES, PREPARE_BYTES, REJECT_BYTES};

    #[test]
    fn test_try_from() {
        assert_eq!(
            Packet::try_from(BytesMut::from(PREPARE_BYTES)).unwrap(),
            Packet::Prepare(PREPARE.clone()),
        );
        assert_eq!(
            Packet::try_from(BytesMut::from(FULFILL_BYTES)).unwrap(),
            Packet::Fulfill(FULFILL.clone()),
        );
        assert_eq!(
            Packet::try_from(BytesMut::from(REJECT_BYTES)).unwrap(),
            Packet::Reject(REJECT.clone()),
        );

        // Empty buffer:
        assert!(Packet::try_from(BytesMut::new()).is_err());
        // Unknown packet type:
        assert!(Packet::try_from(BytesMut::from(&[0x99][..])).is_err());
    }

    #[test]
    fn test_into_bytes_mut() {
        assert_eq!(
            BytesMut::from(Packet::Prepare(PREPARE.clone())),
            BytesMut::from(PREPARE_BYTES),
        );
        assert_eq!(
            BytesMut::from(Packet::Fulfill(FULFILL.clone())),
            BytesMut::from(FULFILL_BYTES),
        );
        assert_eq!(
            BytesMut::from(Packet::Reject(REJECT.clone())),
            BytesMut::from(REJECT_BYTES),
        );
    }
}

#[cfg(test)]
mod test_prepare {
    use super::*;
    use crate::fixtures::{self, PREPARE, PREPARE_BUILDER, PREPARE_BYTES};

    #[test]
    fn test_invalid_address() {
        let mut prep = BytesMut::from(PREPARE_BYTES);
        prep[67] = 42; // convert a byte from the address to a junk character
        assert!(Prepare::try_from(prep).is_err());
    }

    #[test]
    fn test_invalid_ts() {
        for i in 12..(12 + 17) {
            let mut prep = BytesMut::from(PREPARE_BYTES);
            prep[i] = 9; // convert a byte from the address to a junk character
            let err = Prepare::try_from(prep).unwrap_err();
            assert_eq!("Invalid Packet: DateTime must be numeric", &err.to_string());
        }
    }

    #[test]
    fn test_try_from() {
        assert_eq!(
            Prepare::try_from(BytesMut::from(PREPARE_BYTES)).unwrap(),
            *PREPARE
        );

        // Incorrect packet type on an otherwise well-formed Prepare.
        assert!(Prepare::try_from({
            let mut with_wrong_type = BytesMut::from(PREPARE_BYTES);
            with_wrong_type[0] = PacketType::Fulfill as u8;
            with_wrong_type
        })
        .is_err());

        // A packet with junk data appened to the end.
        let with_junk_data = Prepare::try_from({
            let mut buffer = BytesMut::from(PREPARE_BYTES);
            buffer.extend_from_slice(&[0x11, 0x12, 0x13]);
            buffer
        });

        // feature = "strict" is used when fuzzing and is tested here to ensure
        // error is returned instead of roundtripping when junk data is added
        #[cfg(feature = "strict")]
        {
            assert_eq!(
                "Invalid Packet: Unexpected outer trailing bytes",
                &with_junk_data.unwrap_err().to_string()
            );
        }

        #[cfg(not(feature = "strict"))]
        {
            let with_junk_data = with_junk_data.unwrap();
            assert_eq!(with_junk_data.amount(), PREPARE.amount());
            assert_eq!(with_junk_data.expires_at(), *fixtures::EXPIRES_AT);
            assert_eq!(
                with_junk_data.execution_condition(),
                fixtures::EXECUTION_CONDITION
            );
            assert_eq!(with_junk_data.destination(), PREPARE.destination());
            assert_eq!(with_junk_data.data(), fixtures::DATA);
        }
    }

    #[test]
    fn test_into_bytes_mut() {
        assert_eq!(BytesMut::from(PREPARE.clone()), PREPARE_BYTES);
    }

    #[test]
    fn test_amount() {
        assert_eq!(PREPARE.amount(), PREPARE_BUILDER.amount);
    }

    #[test]
    fn test_set_amount() {
        let target_amount = PREPARE_BUILDER.amount;
        let destination = PREPARE_BUILDER.destination.clone();
        let mut prepare = PrepareBuilder {
            amount: 9999,
            destination,
            ..*PREPARE_BUILDER
        }
        .build();
        prepare.set_amount(target_amount);
        assert_eq!(prepare.amount(), target_amount);
        assert_eq!(BytesMut::from(prepare), PREPARE_BYTES);
    }

    #[test]
    fn test_expires_at() {
        assert_eq!(PREPARE.expires_at(), *fixtures::EXPIRES_AT);
    }

    #[test]
    fn test_set_expires_at() {
        let target_expiry = PREPARE_BUILDER.expires_at;
        let destination = PREPARE_BUILDER.destination.clone();
        let mut prepare = PrepareBuilder {
            expires_at: SystemTime::now(),
            destination,
            ..*PREPARE_BUILDER
        }
        .build();
        prepare.set_expires_at(target_expiry);
        assert_eq!(prepare.expires_at(), target_expiry);
        assert_eq!(BytesMut::from(prepare), PREPARE_BYTES);
    }

    #[test]
    fn test_execution_condition() {
        assert_eq!(PREPARE.execution_condition(), fixtures::EXECUTION_CONDITION,);
    }

    #[test]
    fn test_data() {
        assert_eq!(PREPARE.data(), fixtures::DATA);
    }

    #[test]
    fn test_into_data() {
        assert_eq!(PREPARE.clone().into_data(), BytesMut::from(PREPARE.data()),);
    }
}

#[cfg(test)]
mod test_fulfill {
    use super::*;
    use crate::fixtures::{self, FULFILL, FULFILL_BYTES};

    #[test]
    fn test_try_from() {
        assert_eq!(
            Fulfill::try_from(BytesMut::from(FULFILL_BYTES)).unwrap(),
            *FULFILL
        );

        // A packet with junk data appened to the end.
        let with_junk_data = Fulfill::try_from({
            let mut buffer = BytesMut::from(FULFILL_BYTES);
            buffer.extend_from_slice(&[0x11, 0x12, 0x13]);
            buffer
        });

        // feature = "strict" is used when fuzzing and is tested here to ensure
        // error is returned instead of roundtripping when junk data is added
        if cfg!(feature = "strict") {
            assert_eq!(
                "Invalid Packet: Unexpected outer trailing bytes",
                &with_junk_data.unwrap_err().to_string()
            );
        } else {
            let with_junk_data = with_junk_data.unwrap();
            assert_eq!(with_junk_data.fulfillment(), fixtures::FULFILLMENT);
            assert_eq!(with_junk_data.data(), fixtures::DATA);
        }

        // Fail to parse a packet missing a data field, even if a VarStr is in
        // the junk data.
        let with_data_in_junk = {
            let mut buffer = BytesMut::with_capacity(1024);
            buffer.put_u8(PacketType::Fulfill as u8);
            buffer.put_var_octet_string_length(32);
            buffer.put_slice(&fixtures::FULFILLMENT[..]);
            buffer.put_var_octet_string(fixtures::DATA);
            buffer
        };
        assert!(Fulfill::try_from(with_data_in_junk).is_err());
    }

    #[test]
    fn reject_try_from_data_with_trailing_bytes() {
        // It should fail to parse a erroneous packet with trailing bytes after the data field.
        // This differ from the test_try_from test in that
        // the trailing bytes are counted in the total length (varlen prefix) of the data
        #[rustfmt::skip]
        let with_trailing_bytes: &[u8] = &[
            // fulfill packet type
            13,
            // varlen octet string length (40)
            // differs here, output has 33 which would make sense, 32 bytes for fulfillment and 1
            // byte for 0-byte var octet string data.
            40,
            // fullfillment (32)
            40, 136, 1, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 164, 255, 14, 13, 136,
            136, 136, 1, 136, 8, 0, 238, 238, 238, 0, 1, 0, 0, 0,
            // data var octet string length (1)
            0,
            // extra trailer which should not be there (7)
            0, 93, 0, 14, 40, 40, 40,
        ];

        let res = Fulfill::try_from(BytesMut::from(with_trailing_bytes));

        if cfg!(feature = "strict") {
            assert_eq!(
                "Invalid Packet: Unexpected inner trailing bytes",
                &res.unwrap_err().to_string()
            );
        } else {
            res.unwrap();
        }
    }

    #[test]
    fn test_into_bytes_mut() {
        assert_eq!(BytesMut::from(FULFILL.clone()), FULFILL_BYTES);
    }

    #[test]
    fn test_fulfillment() {
        assert_eq!(FULFILL.fulfillment(), fixtures::FULFILLMENT);
    }

    #[test]
    fn test_data() {
        assert_eq!(FULFILL.data(), fixtures::DATA);
    }

    #[test]
    fn test_into_data() {
        assert_eq!(FULFILL.clone().into_data(), BytesMut::from(FULFILL.data()),);
    }
}

#[cfg(test)]
mod test_reject {
    use super::*;
    use crate::fixtures::{self, REJECT, REJECT_BUILDER, REJECT_BYTES};

    #[test]
    fn test_try_from() {
        assert_eq!(
            Reject::try_from(BytesMut::from(REJECT_BYTES)).unwrap(),
            *REJECT,
        );

        // A packet with junk data appened to the end.
        let with_junk_data = Reject::try_from({
            let mut buffer = BytesMut::from(REJECT_BYTES);
            buffer.extend_from_slice(&[0x11, 0x12, 0x13]);
            buffer
        });

        // feature = "strict" is used when fuzzing and is tested here to ensure
        // error is returned instead of roundtripping when junk data is added
        if cfg!(feature = "strict") {
            assert_eq!(
                "Invalid Packet: Unexpected outer trailing bytes",
                &with_junk_data.unwrap_err().to_string()
            );
        } else {
            let with_junk_data = with_junk_data.unwrap();
            assert_eq!(with_junk_data.code(), REJECT_BUILDER.code);
            assert_eq!(with_junk_data.message(), REJECT_BUILDER.message);
            assert_eq!(
                with_junk_data.triggered_by().as_ref(),
                REJECT_BUILDER.triggered_by
            );
            assert_eq!(with_junk_data.data(), fixtures::DATA);
        }
    }

    #[test]
    fn test_into_bytes_mut() {
        assert_eq!(BytesMut::from(REJECT.clone()), REJECT_BYTES);
    }

    #[test]
    fn test_code() {
        assert_eq!(REJECT.code(), REJECT_BUILDER.code);
    }

    #[test]
    fn test_message() {
        assert_eq!(REJECT.message(), REJECT_BUILDER.message);
    }

    #[test]
    fn test_triggered_by() {
        assert_eq!(REJECT.triggered_by().as_ref(), REJECT_BUILDER.triggered_by);
    }

    #[test]
    fn test_data() {
        assert_eq!(REJECT.data(), fixtures::DATA);
    }

    #[test]
    fn test_into_data() {
        assert_eq!(REJECT.clone().into_data(), BytesMut::from(REJECT.data()));
    }
}

#[cfg(test)]
mod test_max_packet_amount_details {
    use super::*;

    static BYTES: &[u8] = b"\
        \x00\x00\x00\x00\x00\x03\x02\x01\
        \x00\x00\x00\x00\x00\x06\x05\x04\
    ";

    static DETAILS: MaxPacketAmountDetails = MaxPacketAmountDetails {
        amount_received: 0x0003_0201,
        max_amount: 0x0006_0504,
    };

    #[test]
    fn test_from_bytes() {
        assert_eq!(MaxPacketAmountDetails::from_bytes(BYTES).unwrap(), DETAILS,);
        assert_eq!(
            MaxPacketAmountDetails::from_bytes(&[][..])
                .unwrap_err()
                .kind(),
            std::io::ErrorKind::UnexpectedEof,
        );
    }

    #[test]
    fn test_to_bytes() {
        assert_eq!(&DETAILS.to_bytes()[..], BYTES);
    }

    #[test]
    fn test_amount_received() {
        assert_eq!(DETAILS.amount_received(), 0x0003_0201);
    }

    #[test]
    fn test_max_amount() {
        assert_eq!(DETAILS.max_amount(), 0x0006_0504);
    }
}
