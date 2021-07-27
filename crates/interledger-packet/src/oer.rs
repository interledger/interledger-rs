#![forbid(unsafe_code)]

use super::errors::{LengthPrefixError, OerError, VarUintError, VariableLengthTimestampError};
use std::convert::TryFrom;
use std::u64;

use bytes::{Buf, BufMut, BytesMut};
use chrono::{TimeZone, Utc};

const HIGH_BIT: u8 = 0x80;
const LOWER_SEVEN_BITS: u8 = 0x7f;
// NOTE: this is stricly different than packet::INTERLEDGER_TIMESTAMP_FORMAT
static VARIABLE_LENGTH_TIMESTAMP_FORMAT: &str = "%Y%m%d%H%M%S%.3fZ";

/// Smallest allowed varlen octets container length, which is 1 byte for `0x00`.
pub const EMPTY_VARLEN_OCTETS_LEN: usize = predict_var_octet_string(0);

/// Length of zero as a variable uint on the wire, `0x01 0x00`.
pub const MIN_VARUINT_LEN: usize = 2;

/// Minimum length of variable length timestamps used in btp on the wire.
pub const MIN_VARLEN_TIMESTAMP_LEN: usize = predict_var_octet_string(15);

/// Returns the size (in bytes) of the buffer that encodes a VarOctetString of
/// `length` bytes.
pub const fn predict_var_octet_string(length: usize) -> usize {
    if length < 128 {
        1 + length
    } else {
        let length_of_length = predict_var_uint_size(length as u64) as usize;
        1 + length_of_length + length
    }
}

/// Returns the minimum number of bytes needed to encode the `value` without leading zeroes in
/// big-endian representation.
pub const fn predict_var_uint_size(value: u64) -> u8 {
    // avoid branching on zero by always orring one; it will not have any effect on other inputs
    let value = value | 1;

    // this will be now be 0..=63 as we handled the zero away
    let highest_bit = 64 - value.leading_zeros();

    // do a highest_bit.ceiling_div(8)
    ((highest_bit + 8 - 1) / 8) as u8
}

pub fn extract_var_octet_string(mut buffer: BytesMut) -> Result<BytesMut, OerError> {
    let buffer_length = buffer.len();
    let mut reader = &buffer[..];
    let content_length = reader.read_var_octet_string_length()?;
    let content_offset = buffer_length - reader.len();

    let mut remaining = buffer.split_off(content_offset);
    if remaining.len() < content_length {
        Err(OerError::UnexpectedEof)
    } else {
        Ok(remaining.split_to(content_length))
    }
}

pub trait BufOerExt<'a> {
    fn peek_var_octet_string(&self) -> Result<&'a [u8], OerError>;
    fn read_var_octet_string(&mut self) -> Result<&'a [u8], OerError>;
    fn skip(&mut self, discard_bytes: usize) -> Result<(), OerError>;
    fn skip_var_octet_string(&mut self) -> Result<(), OerError>;
    fn read_var_octet_string_length(&mut self) -> Result<usize, OerError>;
    fn read_var_uint(&mut self) -> Result<u64, OerError>;

    /// Decodes a variable length timestamp according to [RFC-0030].
    ///
    /// [RFC-0030]: https://github.com/interledger/rfcs/blob/2473d2963a65e5534076c483f3c08a81b8e0cc88/0030-notes-on-oer-encoding/0030-notes-on-oer-encoding.md#variable-length-timestamps
    fn read_variable_length_timestamp(&mut self) -> Result<VariableLengthTimestamp, OerError>;
}

impl<'a> BufOerExt<'a> for &'a [u8] {
    /// Decodes variable-length octet string buffer without moving the cursor.
    #[inline]
    fn peek_var_octet_string(&self) -> Result<&'a [u8], OerError> {
        let mut peek = &self[..];
        let actual_length = peek.read_var_octet_string_length()?;
        let offset = self.len() - peek.len();
        if peek.len() < actual_length {
            Err(OerError::UnexpectedEof)
        } else {
            Ok(&self[offset..(offset + actual_length)])
        }
    }

    /// Decodes variable-length octet string.
    #[inline]
    fn read_var_octet_string(&mut self) -> Result<&'a [u8], OerError> {
        let actual_length = self.read_var_octet_string_length()?;
        if self.len() < actual_length {
            Err(OerError::UnexpectedEof)
        } else {
            let to_return = &self[..actual_length];
            *self = &self[actual_length..];
            Ok(to_return)
        }
    }

    #[inline]
    fn skip(&mut self, discard_bytes: usize) -> Result<(), OerError> {
        if self.len() < discard_bytes {
            Err(OerError::UnexpectedEof)
        } else {
            *self = &self[discard_bytes..];
            Ok(())
        }
    }

    #[inline]
    fn skip_var_octet_string(&mut self) -> Result<(), OerError> {
        let actual_length = self.read_var_octet_string_length()?;
        self.skip(actual_length)
    }

    #[doc(hidden)]
    #[inline]
    fn read_var_octet_string_length(&mut self) -> Result<usize, OerError> {
        if self.remaining() < 1 {
            return Err(OerError::UnexpectedEof);
        }
        let length = self.get_u8();
        if length & HIGH_BIT != 0 {
            let length_prefix_length = (length & LOWER_SEVEN_BITS) as usize;
            // TODO check for canonical length
            if length_prefix_length > 8 {
                Err(LengthPrefixError::TooLarge.into())
            } else if length_prefix_length == 0 {
                Err(LengthPrefixError::IndefiniteLength.into())
            } else {
                if self.len() < length_prefix_length {
                    return Err(OerError::UnexpectedEof);
                }

                let uint = self.get_uint(length_prefix_length);

                check_no_leading_zeroes(length_prefix_length, uint)?;

                if length_prefix_length == 1 && uint < 128 {
                    // Leading zero should not be used
                    // https://github.com/interledger/rfcs/blob/master/0030-notes-on-oer-encoding/0030-notes-on-oer-encoding.md#variable-length-unsigned-integer
                    return Err(LengthPrefixError::LeadingZeros.into());
                }

                // it makes no sense for a length to be u64 but usize, and even that is quite a lot
                let uint = usize::try_from(uint).map_err(|_| LengthPrefixError::UsizeOverflow)?;

                Ok(uint)
            }
        } else {
            Ok(length as usize)
        }
    }

    /// Decodes variable-length octet unsigned integer to get `u64`.
    #[inline]
    fn read_var_uint(&mut self) -> Result<u64, OerError> {
        let size = self.read_var_octet_string_length()?;
        if size == 0 {
            Err(VarUintError::ZeroLength.into())
        } else if size > 8 {
            Err(VarUintError::TooLarge.into())
        } else {
            if self.len() < size {
                return Err(OerError::UnexpectedEof);
            }
            let uint = self.get_uint(size);

            check_no_leading_zeroes(size, uint)?;

            Ok(uint)
        }
    }

    fn read_variable_length_timestamp(&mut self) -> Result<VariableLengthTimestamp, OerError> {
        use once_cell::sync::OnceCell;
        use regex::bytes::Regex;

        static RE: OnceCell<Regex> = OnceCell::new();
        let re = RE.get_or_init(|| Regex::new(r"^[0-9]{4}[0-9]{2}{5}(\.[0-9]{1,3})?Z$").unwrap());

        let octets = self.read_var_octet_string()?;

        let len = match octets.len() {
            15 | 17 | 18 | 19 => Ok(octets.len() as u8),
            x => Err(VariableLengthTimestampError::InvalidLength(x)),
        }?;

        if !re.is_match(octets) {
            return Err(VariableLengthTimestampError::InvalidTimestamp.into());
        }

        // the string might still have bad date in it
        let s = std::str::from_utf8(octets)
            .expect("re matches only ascii, utf8 conversion must succeed");

        let ts = Utc
            .datetime_from_str(s, VARIABLE_LENGTH_TIMESTAMP_FORMAT)
            .map_err(|_| VariableLengthTimestampError::InvalidTimestamp)?;

        Ok(VariableLengthTimestamp { inner: ts, len })
    }
}

#[derive(Debug, PartialEq, Clone)]
pub struct VariableLengthTimestamp {
    inner: chrono::DateTime<chrono::Utc>,
    len: u8,
}

impl VariableLengthTimestamp {
    /// Returns a full length timestamp of the value parsed as RFC3339.
    pub fn parse_from_rfc3339(s: &str) -> std::result::Result<Self, chrono::ParseError> {
        Ok(VariableLengthTimestamp {
            inner: chrono::DateTime::parse_from_rfc3339(s)?.with_timezone(&Utc),
            len: 19,
        })
    }
}

use std::fmt;

impl fmt::Display for VariableLengthTimestamp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        use chrono::Timelike;

        // first let chrono format without the fractions and ending Z
        fmt::Display::fmt(&self.inner.format("%Y%m%d%H%M%S"), f)?;

        // then depending on the length add enough fractions (chrono doesn't support %.1f or %.2f)
        let nanos = self.inner.time().nanosecond() % 1_000_000_000;
        match self.len {
            15 => Ok(()),
            17 => write!(f, ".{:01}", nanos / 100_000_000),
            18 => write!(f, ".{:02}", nanos / 10_000_000),
            19 => write!(f, ".{:03}", nanos / 1_000_000),
            x => unreachable!("should not have timestamp of length: {}", x),
        }?;

        // all varlen timestamps end with Z
        write!(f, "Z")
    }
}

fn check_no_leading_zeroes(_size_on_wire: usize, _uint: u64) -> Result<(), LengthPrefixError> {
    #[cfg(feature = "strict")]
    if _size_on_wire != predict_var_uint_size(_uint) as usize {
        // if we dont check for this fuzzing roundtrip tests will break, as there are
        // "128 | 1, x" class of inputs, where "x" = 0..128
        return Err(LengthPrefixError::StrictLeadingZeros);
    }
    Ok(())
}

pub trait MutBufOerExt: BufMut + Sized {
    /// Encodes bytes as variable-length octet encoded string and puts it into `Buf`.
    #[inline]
    fn put_var_octet_string<B: Buf>(&mut self, buf: B) {
        self.put_var_octet_string_length(buf.remaining());
        self.put(buf);
    }

    /// Encodes the length of a datatype as variable-length octet encoded unsigned integer and puts it into `Buf`
    #[inline]
    fn put_var_octet_string_length(&mut self, length: usize) {
        if length < 128 {
            self.put_u8(length as u8);
        } else {
            let length_of_length = predict_var_uint_size(length as u64) as usize;
            self.put_u8(HIGH_BIT | length_of_length as u8);
            self.put_uint(length as u64, length_of_length);
        }
    }

    /// Encodes `u64` as variable-length octet encoded unsigned integer and puts it into `BufMut`
    #[inline]
    fn put_var_uint(&mut self, uint: u64) {
        let size = predict_var_uint_size(uint) as usize;
        self.put_var_octet_string_length(size);
        self.put_uint(uint, size);
    }

    /// Encodes the given timestamp per the rules, see
    /// [`BufOerExt::read_variable_length_timestamp`].
    fn put_variable_length_timestamp(&mut self, vts: &VariableLengthTimestamp) {
        use std::io::Write;

        self.put_var_octet_string_length(vts.len as usize);
        write!(self.writer(), "{}", vts)
            .expect("BufMut should expand and formatting should never fail");
    }
}

impl<B: BufMut + Sized> MutBufOerExt for B {}

#[cfg(test)]
mod test_functions {
    use bytes::BytesMut;

    use super::fixtures::*;
    use super::*;

    #[test]
    fn test_predict_var_octet_string() {
        let zeroes = &[0; 4096][..];
        let mut buffer = BytesMut::with_capacity(5000);
        for i in 0..4096 {
            buffer.clear();
            buffer.put_var_octet_string(&zeroes[..i]);
            assert_eq!(
                predict_var_octet_string(i),
                buffer.len(),
                "string_length={:?}",
                i,
            );
        }
    }

    #[test]
    fn test_predict_var_uint_size() {
        assert_eq!(predict_var_uint_size(0), 1);
        assert_eq!(predict_var_uint_size(1), 1);
        assert_eq!(predict_var_uint_size(0xff), 1);
        assert_eq!(predict_var_uint_size(0xff + 1), 2);
        assert_eq!(predict_var_uint_size(u64::MAX - 1), 8);
        assert_eq!(predict_var_uint_size(u64::MAX), 8);
    }

    #[test]
    fn test_extract_var_octet_string() {
        assert_eq!(
            extract_var_octet_string(BytesMut::from(TWO_BYTE_VARSTR)).unwrap(),
            BytesMut::from(&TWO_BYTE_VARSTR[1..3]),
        );
        assert_eq!(
            extract_var_octet_string(BytesMut::new()).unwrap_err(),
            OerError::UnexpectedEof,
        );
        assert_eq!(
            extract_var_octet_string(BytesMut::from(LENGTH_TOO_HIGH_VARSTR)).unwrap_err(),
            OerError::UnexpectedEof,
        );
    }
}

#[cfg(test)]
mod test_buf_oer_ext {
    use once_cell::sync::Lazy;

    use super::fixtures::*;
    use super::*;

    // These bufferes have their lengths encoded in multiple bytes.
    static SIZE_128_VARSTR: Lazy<Vec<u8>> = Lazy::new(|| {
        let mut data = vec![0x81, 0x80];
        data.extend(&[0x00; 128][..]);
        data
    });
    static SIZE_5678_VARSTR: Lazy<Vec<u8>> = Lazy::new(|| {
        let mut data = vec![0x82, 0x16, 0x2e];
        data.extend(&[0x00; 5678][..]);
        data
    });

    #[test]
    fn test_peek_var_octet_string() {
        let tests: &[(&[u8], &[u8])] = &[
            (&[0x00], &[]),
            (ZERO_LENGTH_VARSTR, &[]),
            (ONE_BYTE_VARSTR, &[0x01]),
            (TWO_BYTE_VARSTR, &[0x01, 0x02]),
            (&SIZE_128_VARSTR, &[0; 128][..]),
            (&SIZE_5678_VARSTR, &[0; 5678][..]),
        ];
        for (buffer, varstr) in tests {
            assert_eq!((&buffer[..]).peek_var_octet_string().unwrap(), *varstr,);
        }

        assert_eq!(
            (&[][..]).peek_var_octet_string().unwrap_err(),
            OerError::UnexpectedEof,
        );
        assert_eq!(
            LENGTH_TOO_HIGH_VARSTR.peek_var_octet_string().unwrap_err(),
            OerError::UnexpectedEof,
        );
    }

    #[test]
    fn test_skip() {
        let mut empty = &[][..];
        assert_eq!(empty.skip(1).unwrap_err(), OerError::UnexpectedEof);

        let mut reader = &[0x01, 0x02, 0x03][..];
        assert!(reader.skip(2).is_ok());
        assert_eq!(reader, &[0x03]);
    }

    #[test]
    fn test_skip_var_octet_string() {
        let tests: &[(Vec<u8>, usize)] = &[
            (ZERO_LENGTH_VARSTR.to_vec(), 1),
            (ONE_BYTE_VARSTR.to_vec(), 2),
            (TWO_BYTE_VARSTR.to_vec(), 3),
            (SIZE_128_VARSTR.clone(), SIZE_128_VARSTR.len()),
            (SIZE_5678_VARSTR.clone(), SIZE_5678_VARSTR.len()),
        ];
        for (buffer, offset) in tests {
            let mut reader = &buffer[..];
            assert!(reader.skip_var_octet_string().is_ok());
            assert_eq!(reader.len(), buffer.len() - *offset);
        }

        assert_eq!(
            (&[][..]).skip_var_octet_string().unwrap_err(),
            OerError::UnexpectedEof,
        );

        // this is a workaround for clippy::redundant_slicing
        let mut cursor = LENGTH_TOO_HIGH_VARSTR;

        assert_eq!(
            cursor.skip_var_octet_string().unwrap_err(),
            OerError::UnexpectedEof
        );
    }

    #[test]
    fn read_too_big_var_octet_string_length() {
        // The length of the octet string takes 1 + 9 bytes, which is too big compared to usize
        // which is the maximum supported.
        let too_big: &[u8] = &[HIGH_BIT | 9, 0, 1, 2, 3, 4, 5, 6, 7, 8];
        for len in 1..=too_big.len() {
            let mut slice = &too_big[..len];
            assert_eq!(
                slice.read_var_octet_string_length().unwrap_err(),
                OerError::LengthPrefix(LengthPrefixError::TooLarge),
                "error should always be too large prefix regardless of the input length"
            );
        }
    }

    #[test]
    fn read_var_octet_string_length_buffer_too_small() {
        // The length of the octet string means 1 + 4 bytes of total length, but only at most 1 + 3
        // can be read.
        let mut too_small = vec![HIGH_BIT | 4];
        too_small.extend(std::iter::repeat(0xff).take(3));
        for len in 0..5 {
            let mut reader = &too_small[..len];
            assert_eq!(
                reader.read_var_octet_string_length().unwrap_err(),
                OerError::UnexpectedEof
            );
        }
    }

    #[test]
    fn read_empty_octet_string() {
        let mut reader = &[0][..];
        let slice = reader.read_var_octet_string().unwrap();
        assert!(slice.is_empty());
    }

    #[test]
    fn read_smaller_octet_string() {
        let mut two_bytes = &[0x01, 0xb0][..];
        assert_eq!(two_bytes.read_var_octet_string().unwrap(), &[0xb0]);
    }

    #[test]
    fn read_longer_length_octet_string() {
        let mut larger = vec![0x82, 0x01, 0x00];
        let larger_string = [0xb0; 256];
        larger.extend(&larger_string);

        let mut reader = &larger[..];

        assert_eq!(reader.read_var_octet_string().unwrap(), &larger_string[..]);
    }

    #[test]
    fn indefinite_len_octets() {
        #[allow(clippy::identity_op)]
        let indefinite = HIGH_BIT | 0x00;

        let mut bytes = &[indefinite, 0x00, 0x01, 0x02][..];

        let e = bytes.read_var_octet_string_length().unwrap_err();

        assert_eq!(
            e,
            OerError::LengthPrefix(LengthPrefixError::IndefiniteLength)
        );
        assert_eq!("indefinite lengths are not allowed", e.to_string());
    }

    #[test]
    fn way_too_long_octets_with_126_bytes_of_length() {
        // this would be quite the long string, not great for network programming
        let mut bytes = vec![HIGH_BIT | 126];
        bytes.extend(std::iter::repeat(0xff).take(125));
        bytes.push(1);

        let mut reader = &bytes[..];

        let e = reader.read_var_octet_string_length().unwrap_err();

        assert_eq!("length prefix too large", e.to_string());
    }

    #[test]
    fn way_too_long_octets_with_9_bytes_of_length() {
        let mut bytes = vec![HIGH_BIT | 9];
        bytes.extend(std::iter::repeat(0xff).take(8));
        bytes.push(1);

        let mut reader = &bytes[..];

        let e = reader.read_var_octet_string_length().unwrap_err();

        assert_eq!("length prefix too large", e.to_string());
    }

    #[test]
    fn max_len_octets() {
        let mut bytes = vec![HIGH_BIT | 4];
        bytes.extend(std::iter::repeat(0xff).take(4));

        // here 84 ff ff ff ff is used as a max usize (in 32-bit systems)

        let mut reader = &bytes[..];

        let len = reader.read_var_octet_string_length().unwrap();

        assert_eq!(u32::MAX, len as u32);
    }

    #[test]
    fn test_read_var_uint() {
        let tests: &[(&[u8], u64, usize)] = &[
            (&[0x01, 0x00], 0, 2),
            (&[0x01, 0x09], 9, 2),
            (&[0x02, 0x01, 0x02], 0x0102, 3),
            (&[0x03, 0x01, 0x02, 0x03], 0x0001_0203, 4),
            (&[0x04, 0x01, 0x02, 0x03, 0x04], 0x0102_0304, 5),
            (&[0x05, 0x01, 0x02, 0x03, 0x04, 0x05], 0x0001_0203_0405, 6),
            (
                &[0x06, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06],
                0x0102_0304_0506,
                7,
            ),
            (
                &[0x07, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07],
                0x0001_0203_0405_0607,
                8,
            ),
            (
                &[0x08, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08],
                0x0102_0304_0506_0708,
                9,
            ),
        ];

        for (buffer, value, offset) in tests {
            let mut reader = &buffer[..];
            let uint = reader.read_var_uint().unwrap();
            assert_eq!(uint, *value);
            assert_eq!(reader.len(), buffer.len() - *offset);
        }

        let tests: &[(&[u8], OerError)] = &[
            // VarUint's must have at least 1 byte.
            (&[0x00], OerError::VarUint(VarUintError::ZeroLength)),
            // Data must be present.
            (&[0x04], OerError::UnexpectedEof),
            // Enough bytes must be present.
            (&[0x04, 0x01, 0x02, 0x03], OerError::UnexpectedEof),
            // Too many bytes.
            (
                &[0x09, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09],
                OerError::VarUint(VarUintError::TooLarge),
            ),
        ];

        for (buffer, oer_error) in tests {
            assert_eq!((&buffer[..]).read_var_uint().unwrap_err(), *oer_error);
        }
    }

    #[test]
    fn peek_too_long_uint() {
        // in interledger-stream there is a use case to accept larger than u64::MAX for a varuint.
        // it is done through peeking for the length of the varuint, using the length > 8 to
        // default to u64::MAX.
        let bytes: &[u8] = &[0x09u8, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x01];

        let reader = bytes;
        assert_eq!(9, reader.peek_var_octet_string().unwrap().len());
    }

    #[test]
    fn read_variable_length_timestamp() {
        let valid: &[(&[u8], &str)] = &[
            (b"20171224161432.279Z", "2017-12-24 16:14:32.279 UTC"),
            (b"20171224161432.27Z", "2017-12-24 16:14:32.270 UTC"),
            (b"20171224161432.2Z", "2017-12-24 16:14:32.200 UTC"),
            (b"20171224161432Z", "2017-12-24 16:14:32 UTC"),
            (b"20161231235960.852Z", "2016-12-31 23:59:60.852 UTC"),
            (b"20171225000000Z", "2017-12-25 00:00:00 UTC"),
            (b"99991224161432.279Z", "9999-12-24 16:14:32.279 UTC"),
        ];

        let mut buffer = BytesMut::with_capacity(1 + valid[0].0.len());

        for &(input, expected) in valid {
            buffer.clear();
            buffer.put_var_octet_string(input);
            let ts = buffer.as_ref().read_variable_length_timestamp().unwrap();
            assert_eq!(ts.len as usize, input.len());
            assert_eq!(expected, ts.inner.to_string());
        }
    }

    #[test]
    fn invalid_variable_length_timestamps() {
        use std::fmt::Write;

        #[rustfmt::skip]
        let invalid: &[(u32, &[u8], &str)] = &[
            (line!(), b"20171224235312.431+0200", "Invalid length for variable length timestamp: 23"),
            (line!(), b"20171224215312.4318Z",    "Invalid length for variable length timestamp: 20"),
            (line!(), b"20171224161432,279Z",     "Input failed to parse as timestamp"),
            (line!(), b"20171324161432.279Z",     "Input failed to parse as timestamp"),
            // (line!(), b"20171224230000.20Z", "according to RFC-0030 this is an invalid timestamp but it doesn't appear to be"),
            (line!(), b"20171224230000.Z",        "Invalid length for variable length timestamp: 16"),
            (line!(), b"20171224240000Z",         "Input failed to parse as timestamp"),
            (line!(), b"2017122421531Z",          "Invalid length for variable length timestamp: 14"),
            (line!(), b"201712242153Z",           "Invalid length for variable length timestamp: 13"),
            (line!(), b"2017122421Z",             "Invalid length for variable length timestamp: 11"),
        ];

        let mut buffer = BytesMut::with_capacity(1 + invalid[0].1.len());
        let mut s = String::new();

        for &(line, example, error) in invalid {
            buffer.clear();
            s.clear();

            buffer.put_var_octet_string(example);
            let e = buffer
                .as_ref()
                .read_variable_length_timestamp()
                .unwrap_err();

            write!(s, "{}", e).unwrap();
            assert_eq!(error, s, "on line {}", line);
        }
    }
}

#[cfg(test)]
mod buf_mut_oer_ext {
    use super::*;

    #[test]
    fn test_put_var_octet_string() {
        let long_varstr = &[0x00; 256][..];
        let long_buffer = {
            let mut buffer = vec![0x82, 0x01, 0x00];
            buffer.extend_from_slice(long_varstr);
            buffer
        };

        let tests: &[(&[u8], &[u8])] = &[
            // Write an empty string.
            (b"", b"\x00"),
            // Write a single byte.
            (b"\xb0", b"\x01\xb0"),
            (long_varstr, &long_buffer[..]),
        ];

        let mut writer = BytesMut::with_capacity(10);

        for (varstr, buffer) in tests {
            writer.clear();
            writer.put_var_octet_string(*varstr);
            assert_eq!(&writer[..], *buffer);
        }
    }

    #[test]
    fn test_put_var_uint() {
        let tests: &[(&[u8], u64, usize)] = &[
            (&[0x01, 0x00], 0, 2),
            (&[0x01, 0x09], 9, 2),
            (&[0x02, 0x01, 0x02], 0x0102, 3),
            (&[0x03, 0x01, 0x02, 0x03], 0x0001_0203, 4),
            (&[0x04, 0x01, 0x02, 0x03, 0x04], 0x0102_0304, 5),
            (&[0x05, 0x01, 0x02, 0x03, 0x04, 0x05], 0x0001_0203_0405, 6),
            (
                &[0x06, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06],
                0x0102_0304_0506,
                7,
            ),
            (
                &[0x07, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07],
                0x0001_0203_0405_0607,
                8,
            ),
            (
                &[0x08, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08],
                0x0102_0304_0506_0708,
                9,
            ),
        ];

        let mut writer = BytesMut::with_capacity(10);

        for (buffer, value, _offset) in tests {
            writer.clear();
            writer.put_var_uint(*value);
            assert_eq!(writer, *buffer);
        }
    }

    #[test]
    fn test_put_variable_length_timestamp() {
        let tests: &[(&[u8], &str)] = &[
            (b"20171224161432.279Z", "20171224161432.279Z"),
            (b"20171224161432.27Z", "20171224161432.27Z"),
            (b"20171224161432.2Z", "20171224161432.2Z"),
            (b"20171224161432Z", "20171224161432Z"),
        ];

        let mut write_buffer = BytesMut::with_capacity(1 + tests[0].0.len());

        for (data, input) in tests {
            write_buffer.clear();

            write_buffer.put_var_octet_string(*data);
            let ts = write_buffer
                .as_ref()
                .read_variable_length_timestamp()
                .unwrap();

            assert_eq!(&ts.to_string(), input);

            write_buffer.clear();
            write_buffer.put_variable_length_timestamp(&ts);

            assert_eq!(data, &write_buffer[1..].as_ref());
        }
    }
}

#[cfg(test)]
mod fixtures {
    pub static ZERO_LENGTH_VARSTR: &[u8] = &[0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06];
    pub static ONE_BYTE_VARSTR: &[u8] = &[0x01, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06];
    pub static TWO_BYTE_VARSTR: &[u8] = &[0x02, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06];
    /// This buffer is an incorrectly-encoded VarString.
    pub static LENGTH_TOO_HIGH_VARSTR: &[u8] = &[0x07, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06];
}
