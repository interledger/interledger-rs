use byteorder::{BigEndian, ReadBytesExt, WriteBytesExt};
use bytes::{Buf, BufMut, Bytes, IntoBuf};
use num_bigint::BigUint;
use std::fmt::Debug;
use std::io::{self, Read, Result, Write};

const HIGH_BIT: u8 = 0x80;
const LOWER_SEVEN_BITS: u8 = 0x7f;

// FIXME: interledger-packet already has a fixed version of this trait, this is extra
pub trait ReadOerExt: Read + ReadBytesExt + Debug {
    #[inline]
    fn read_var_octet_string(&mut self) -> Result<Vec<u8>> {
        let length = self.read_var_octet_string_length()?;

        // TODO handle if the length is too long
        let mut buf = Vec::with_capacity(length as usize);
        self.take(length).read_to_end(&mut buf)?;
        Ok(buf)
    }

    fn read_var_octet_string_length(&mut self) -> Result<u64> {
        let length: u8 = self.read_u8()?;

        if length == 0 {
            return Ok(0);
        }

        let actual_length: u64 = if length & HIGH_BIT != 0 {
            let length_prefix_length = length & LOWER_SEVEN_BITS;
            if length_prefix_length == 0 {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "Indefinite lengths are not allowed",
                ));
            } else if length_prefix_length > 8 {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!("Length of length out of range: {}", length_prefix_length),
                ));
            }
            self.read_uint::<BigEndian>(length_prefix_length as usize)? as u64
        } else {
            u64::from(length)
        };

        Ok(actual_length)
    }

    #[inline]
    fn read_var_uint(&mut self) -> Result<BigUint> {
        let contents = self.read_var_octet_string()?;
        Ok(BigUint::from_bytes_be(&contents))
    }
}

// Add this trait to all Readable things when this is used
impl<R: io::Read + ?Sized + Debug> ReadOerExt for R {}

pub trait WriteOerExt: Write + WriteBytesExt + Debug {
    #[inline]
    fn write_var_octet_string(&mut self, string: &[u8]) -> Result<()> {
        let length = string.len();

        // FIXME: this is duplicate of interledger-packet/src/oer.rs like the rest of the file

        if length < 128 {
            self.write_u8(length as u8)?;
        } else {
            let len_of_len = interledger_packet::oer::predict_var_uint_size(length as u64);
            self.write_u8(HIGH_BIT | len_of_len as u8)?;
            self.write_uint::<BigEndian>(length as u64, len_of_len as usize)?;
        }
        self.write_all(string)?;
        Ok(())
    }

    #[inline]
    // Write a u64 as an OER VarUInt
    fn write_var_uint(&mut self, uint: &BigUint) -> Result<()> {
        self.write_var_octet_string(&uint.to_bytes_be())?;
        Ok(())
    }
}

// Add this trait to all Writable things when this is used
impl<W: io::Write + ?Sized + Debug> WriteOerExt for W {}

pub trait BufOerExt: Buf + Sized {
    #[inline]
    // TODO should this return a Bytes type or a Buf?
    fn get_var_octet_string(&mut self) -> Bytes {
        let length: u8 = self.get_u8();

        if length == 0 {
            return Bytes::new();
        }

        let actual_length: usize = if length & HIGH_BIT != 0 {
            let length_prefix_length = length & LOWER_SEVEN_BITS;
            // TODO check for canonical length
            self.get_uint_be(length_prefix_length as usize) as usize
        } else {
            length as usize
        };

        // TODO handle if the length is too long
        let buf = Bytes::from(self.bytes()).slice_to(actual_length);
        self.advance(actual_length);
        buf
    }

    #[inline]
    fn get_var_uint(&mut self) -> BigUint {
        let contents = self.get_var_octet_string();
        BigUint::from_bytes_be(&contents[..])
    }
}

impl<B: Buf + Sized> BufOerExt for B {}

pub trait MutBufOerExt: BufMut + Sized {
    #[inline]
    fn put_var_octet_string<B>(&mut self, buf: B)
    where
        B: IntoBuf,
    {
        let buf = buf.into_buf();
        let length = buf.remaining();

        if length < 127 {
            self.put_u8(length as u8);
        } else {
            let bit_length_of_length = format!("{:b}", length).chars().count();
            let length_of_length = { bit_length_of_length as f32 / 8.0 }.ceil() as u8;
            self.put_u8(HIGH_BIT | length_of_length);
            self.put_uint_be(length as u64, length_of_length as usize);
        }
        self.put(buf);
    }

    #[inline]
    // Write a u64 as an OER VarUInt
    fn put_var_uint(&mut self, uint: &BigUint) {
        self.put_var_octet_string(uint.to_bytes_be());
    }
}

impl<B: BufMut + Sized> MutBufOerExt for B {}

#[cfg(test)]
mod writer_ext {
    use super::*;

    #[test]
    fn it_writes_var_octet_strings() {
        let mut empty = vec![];
        empty.write_var_octet_string(&[]).unwrap();
        assert_eq!(empty, vec![0]);

        let mut one = vec![];
        one.write_var_octet_string(&[0xb0]).unwrap();
        assert_eq!(one, vec![0x01, 0xb0]);

        let mut larger = vec![];
        let larger_string: Vec<u8> = vec![0xb0; 256];
        larger.write_var_octet_string(&larger_string).unwrap();
        let mut expected = vec![0x82, 0x01, 0x00];
        expected.extend(larger_string);
        assert_eq!(larger.len(), 259);
        assert_eq!(larger, expected);
    }
}

#[cfg(test)]
mod reader_ext {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn it_reads_var_octet_strings() {
        let nothing = vec![0];
        assert_eq!(
            Cursor::new(nothing).read_var_octet_string().unwrap().len(),
            0
        );

        let two_bytes = vec![0x01, 0xb0];
        assert_eq!(
            Cursor::new(two_bytes).read_var_octet_string().unwrap(),
            &[0xb0]
        );

        let mut larger = vec![0x82, 0x01, 0x00];
        let larger_string: Vec<u8> = vec![0xb0; 256];
        larger.extend(&larger_string);
        assert_eq!(
            Cursor::new(larger).read_var_octet_string().unwrap(),
            &larger_string[..]
        );
    }

    #[test]
    fn indefinite_len_octets() {
        // FIXME: couldn't find the ending octets but this encoding is not at least introduced in
        // the
        // https://github.com/interledger/rfcs/blob/master/0030-notes-on-oer-encoding/0030-notes-on-oer-encoding.md#short-form
        // so we have to assume it wasn't meant to be supported.
        let bytes = vec![0x80u8, 0x00, 0x01, 0x02];

        let e = Cursor::new(bytes)
            .read_var_octet_string_length()
            .unwrap_err();

        assert_eq!("Indefinite lengths are not allowed", e.to_string());
    }

    #[test]
    fn way_too_long_octets_with_126_bytes_of_length() {
        // this would be quite the long string, not great for network programming
        let mut bytes = vec![(128 | 126)];
        bytes.extend(std::iter::repeat(0xff).take(125));
        bytes.push(1);

        let e = Cursor::new(bytes)
            .read_var_octet_string_length()
            .unwrap_err();

        assert_eq!("Length of length out of range: 126", e.to_string());
    }

    #[test]
    fn way_too_long_octets_with_9_bytes_of_length() {
        let mut bytes = vec![(128 | 9)];
        bytes.extend(std::iter::repeat(0xff).take(8));
        bytes.push(1);

        let e = Cursor::new(bytes)
            .read_var_octet_string_length()
            .unwrap_err();

        assert_eq!("Length of length out of range: 9", e.to_string());
    }

    #[test]
    fn max_len_octets() {
        let mut bytes = vec![(128 | 8)];
        bytes.extend(std::iter::repeat(0xff).take(8));

        // in reality the limit doesn't need to be over u32, or u16 or possibly even u8 range, this
        // u64::MAX length octet stream would be quite the chore to process.

        let len = Cursor::new(bytes).read_var_octet_string_length().unwrap();

        assert_eq!(u64::MAX, len);
    }
}
