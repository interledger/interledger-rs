#![forbid(unsafe_code)]

use std::io::{Error, ErrorKind, Result};
use std::u64;

use byteorder::{BigEndian, ReadBytesExt};
use bytes::{Buf, BufMut, BytesMut, IntoBuf};

const HIGH_BIT: u8 = 0x80;
const LOWER_SEVEN_BITS: u8 = 0x7f;

/// Returns the size (in bytes) of the buffer that encodes a VarOctetString of
/// `length` bytes.
pub fn predict_var_octet_string(length: usize) -> usize {
    if length < 128 {
        1 + length
    } else {
        let length_of_length = predict_var_uint_size(length as u64);
        1 + length_of_length + length
    }
}

/// Returns the minimum number of bytes needed to encode the value.
/// Returns an error of the value requires more than 8 bytes.
fn predict_var_uint_size(value: u64) -> usize {
    for i in 1..=8 {
        let max = u64::MAX >> (64 - 8 * i);
        if value <= max {
            return i;
        }
    }
    unreachable!()
}

pub fn extract_var_octet_string(mut buffer: BytesMut) -> Result<BytesMut> {
    let buffer_length = buffer.len();
    let mut reader = &buffer[..];
    let content_length = reader.read_var_octet_string_length()?;
    let content_offset = buffer_length - reader.len();

    let mut remaining = buffer.split_off(content_offset);
    if remaining.len() < content_length {
        Err(Error::new(ErrorKind::UnexpectedEof, "buffer too small"))
    } else {
        Ok(remaining.split_to(content_length))
    }
}

pub trait BufOerExt<'a> {
    fn peek_var_octet_string(&self) -> Result<&'a [u8]>;
    fn read_var_octet_string(&mut self) -> Result<&'a [u8]>;
    fn skip(&mut self, discard_bytes: usize) -> Result<()>;
    fn skip_var_octet_string(&mut self) -> Result<()>;
    fn read_var_octet_string_length(&mut self) -> Result<usize>;
    fn read_var_uint(&mut self) -> Result<u64>;
}

impl<'a> BufOerExt<'a> for &'a [u8] {
    /// Decodes variable-length octet string buffer without moving the cursor.
    #[inline]
    fn peek_var_octet_string(&self) -> Result<&'a [u8]> {
        let mut peek = &self[..];
        let actual_length = peek.read_var_octet_string_length()?;
        let offset = self.len() - peek.len();
        if peek.len() < actual_length {
            Err(Error::new(ErrorKind::UnexpectedEof, "buffer too small"))
        } else {
            Ok(&self[offset..(offset + actual_length)])
        }
    }

    /// Decodes variable-length octet string.
    #[inline]
    fn read_var_octet_string(&mut self) -> Result<&'a [u8]> {
        let actual_length = self.read_var_octet_string_length()?;
        if self.len() < actual_length {
            Err(Error::new(ErrorKind::UnexpectedEof, "buffer too small"))
        } else {
            let to_return = &self[..actual_length];
            *self = &self[actual_length..];
            Ok(to_return)
        }
    }

    #[inline]
    fn skip(&mut self, discard_bytes: usize) -> Result<()> {
        if self.len() < discard_bytes {
            Err(Error::new(ErrorKind::UnexpectedEof, "buffer too small"))
        } else {
            *self = &self[discard_bytes..];
            Ok(())
        }
    }

    #[inline]
    fn skip_var_octet_string(&mut self) -> Result<()> {
        let actual_length = self.read_var_octet_string_length()?;
        self.skip(actual_length)
    }

    #[doc(hidden)]
    #[inline]
    fn read_var_octet_string_length(&mut self) -> Result<usize> {
        let length = self.read_u8()?;
        if length & HIGH_BIT != 0 {
            let length_prefix_length = (length & LOWER_SEVEN_BITS) as usize;
            // TODO check for canonical length
            if length_prefix_length > 8 {
                Err(Error::new(
                    ErrorKind::InvalidData,
                    "length prefix too large",
                ))
            } else {
                Ok(self.read_uint::<BigEndian>(length_prefix_length)? as usize)
            }
        } else {
            Ok(length as usize)
        }
    }

    /// Decodes variable-length octet unsigned integer to get `u64`.
    #[inline]
    fn read_var_uint(&mut self) -> Result<u64> {
        let size = self.read_var_octet_string_length()?;
        if size == 0 {
            Err(Error::new(ErrorKind::InvalidData, "zero-length VarUInt"))
        } else if size > 8 {
            Err(Error::new(ErrorKind::InvalidData, "VarUInt too large"))
        } else {
            Ok(self.read_uint::<BigEndian>(size)?)
        }
    }
}

pub trait MutBufOerExt: BufMut + Sized {
    /// Encodes bytes as variable-length octet encoded string and puts it into `Buf`.
    #[inline]
    fn put_var_octet_string<B>(&mut self, buf: B)
    where
        B: IntoBuf,
    {
        let buf = buf.into_buf();
        self.put_var_octet_string_length(buf.remaining());
        self.put(buf);
    }

    #[doc(hidden)]
    #[inline]
    fn put_var_octet_string_length(&mut self, length: usize) {
        if length < 128 {
            self.put_u8(length as u8);
        } else {
            let length_of_length = predict_var_uint_size(length as u64);
            self.put_u8(HIGH_BIT | length_of_length as u8);
            self.put_uint_be(length as u64, length_of_length);
        }
    }

    /// Encodes `u64` as variable-length octet encoded unsigned integer and puts it into `Buf`
    #[inline]
    fn put_var_uint(&mut self, uint: u64) {
        let size = predict_var_uint_size(uint);
        self.put_var_octet_string_length(size);
        self.put_uint_be(uint, size);
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
            extract_var_octet_string(BytesMut::new())
                .unwrap_err()
                .kind(),
            ErrorKind::UnexpectedEof,
        );
        assert_eq!(
            extract_var_octet_string(BytesMut::from(LENGTH_TOO_HIGH_VARSTR))
                .unwrap_err()
                .kind(),
            ErrorKind::UnexpectedEof,
        );
    }
}

#[cfg(test)]
mod test_buf_oer_ext {
    use lazy_static::lazy_static;

    use super::fixtures::*;
    use super::*;

    lazy_static! {
        // These bufferes have their lengths encoded in multiple bytes.
        static ref SIZE_128_VARSTR: Vec<u8> = {
            let mut data = vec![0x81, 0x80];
            data.extend(&[0x00; 128][..]);
            data
        };
        static ref SIZE_5678_VARSTR: Vec<u8> = {
            let mut data = vec![0x82, 0x16, 0x2e];
            data.extend(&[0x00; 5678][..]);
            data
        };
    }

    #[test]
    fn test_peek_var_octet_string() {
        let tests: &[(Vec<u8>, &[u8])] = &[
            (vec![0x00], &[]),
            (ZERO_LENGTH_VARSTR.to_vec(), &[]),
            (ONE_BYTE_VARSTR.to_vec(), &[0x01]),
            (TWO_BYTE_VARSTR.to_vec(), &[0x01, 0x02]),
            (SIZE_128_VARSTR.clone(), &[0; 128][..]),
            (SIZE_5678_VARSTR.clone(), &[0; 5678][..]),
        ];
        for (buffer, varstr) in tests {
            assert_eq!((&buffer[..]).peek_var_octet_string().unwrap(), *varstr,);
        }

        assert_eq!(
            (&[][..]).peek_var_octet_string().unwrap_err().kind(),
            ErrorKind::UnexpectedEof,
        );
        assert_eq!(
            LENGTH_TOO_HIGH_VARSTR
                .peek_var_octet_string()
                .unwrap_err()
                .kind(),
            ErrorKind::UnexpectedEof,
        );
    }

    #[test]
    fn test_skip() {
        let mut empty = &[][..];
        assert_eq!(empty.skip(1).unwrap_err().kind(), ErrorKind::UnexpectedEof,);

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
            (&[][..]).skip_var_octet_string().unwrap_err().kind(),
            ErrorKind::UnexpectedEof,
        );

        assert_eq!(
            (&LENGTH_TOO_HIGH_VARSTR[..])
                .skip_var_octet_string()
                .unwrap_err()
                .kind(),
            ErrorKind::UnexpectedEof,
        );
    }

    #[test]
    fn test_read_var_octet_string_length() {
        // The length of the octet string fits in 9 bytes, which is too big.
        let mut too_big: &[u8] = &[HIGH_BIT | 0x09];
        assert_eq!(
            too_big.read_var_octet_string_length().unwrap_err().kind(),
            ErrorKind::InvalidData,
        );
    }

    #[test]
    fn test_read_var_uint() {
        let tests: &[(Vec<u8>, u64, usize)] = &[
            (vec![0x01, 0x00], 0, 2),
            (vec![0x01, 0x09], 9, 2),
            (vec![0x02, 0x01, 0x02], 0x0102, 3),
            (vec![0x03, 0x01, 0x02, 0x03], 0x0001_0203, 4),
            (vec![0x04, 0x01, 0x02, 0x03, 0x04], 0x0102_0304, 5),
            (
                vec![0x05, 0x01, 0x02, 0x03, 0x04, 0x05],
                0x0001_0203_0405,
                6,
            ),
            (
                vec![0x06, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06],
                0x0102_0304_0506,
                7,
            ),
            (
                vec![0x07, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07],
                0x0001_0203_0405_0607,
                8,
            ),
            (
                vec![0x08, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08],
                0x0102_0304_0506_0708,
                9,
            ),
        ];

        for (buffer, value, offset) in tests {
            let mut reader = &buffer[..];
            assert_eq!(reader.read_var_uint().unwrap(), *value);
            assert_eq!(reader.len(), buffer.len() - *offset);
        }

        let tests: &[(Vec<u8>, ErrorKind)] = &[
            // VarUint's must have at least 1 byte.
            (vec![0x00], ErrorKind::InvalidData),
            // Data must be present.
            (vec![0x04], ErrorKind::UnexpectedEof),
            // Enough bytes must be present.
            (vec![0x04, 0x01, 0x02, 0x03], ErrorKind::UnexpectedEof),
            // Too many bytes.
            (
                vec![0x09, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09],
                ErrorKind::InvalidData,
            ),
        ];

        for (buffer, error_kind) in tests {
            assert_eq!(
                (&buffer[..]).read_var_uint().unwrap_err().kind(),
                *error_kind,
            );
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
            buffer.extend_from_slice(&long_varstr);
            buffer
        };

        let tests: &[(&[u8], &[u8])] = &[
            // Write an empty string.
            (b"", b"\x00"),
            // Write a single byte.
            (b"\xb0", b"\x01\xb0"),
            (long_varstr, &long_buffer[..]),
        ];

        for (varstr, buffer) in tests {
            let mut writer = Vec::new();
            writer.put_var_octet_string(varstr);
            assert_eq!(&writer[..], *buffer);
        }
    }

    #[test]
    fn test_put_var_uint() {
        let tests: &[(Vec<u8>, u64, usize)] = &[
            (vec![0x01, 0x00], 0, 2),
            (vec![0x01, 0x09], 9, 2),
            (vec![0x02, 0x01, 0x02], 0x0102, 3),
            (vec![0x03, 0x01, 0x02, 0x03], 0x0001_0203, 4),
            (vec![0x04, 0x01, 0x02, 0x03, 0x04], 0x0102_0304, 5),
            (
                vec![0x05, 0x01, 0x02, 0x03, 0x04, 0x05],
                0x0001_0203_0405,
                6,
            ),
            (
                vec![0x06, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06],
                0x0102_0304_0506,
                7,
            ),
            (
                vec![0x07, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07],
                0x0001_0203_0405_0607,
                8,
            ),
            (
                vec![0x08, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08],
                0x0102_0304_0506_0708,
                9,
            ),
        ];

        for (buffer, value, _offset) in tests {
            let mut writer = Vec::new();
            writer.put_var_uint(*value);
            assert_eq!(writer, *buffer);
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
