use byteorder::{BigEndian, ByteOrder, ReadBytesExt, WriteBytesExt};
use bytes::{Buf, BufMut, Bytes, IntoBuf};
use std::fmt::Debug;
use std::io::{self, Read, Result, Write};

const HIGH_BIT: u8 = 0x80;
const LOWER_SEVEN_BITS: u8 = 0x7f;

pub trait ReadOerExt: Read + ReadBytesExt + Debug {
    #[inline]
    /// Decodes variable-length octet encoded string and reads to rdr.
    fn read_var_octet_string(&mut self) -> Result<Vec<u8>> {
        // TODO: check for max size?
        // TODO: check for no content?
        let length: u8 = self.read_u8()?;

        if length == 0 {
            return Ok(vec![]);
        }

        let actual_length: u64 = if length & HIGH_BIT != 0 {
            let length_prefix_length = length & LOWER_SEVEN_BITS;
            // TODO check for canonical length
            self.read_uint::<BigEndian>(length_prefix_length as usize)? as u64
        } else {
            u64::from(length)
        };

        let mut buf = Vec::with_capacity(actual_length as usize);
        self.take(actual_length).read_to_end(&mut buf)?;
        Ok(buf)
    }

    #[inline]
    /// Decodes variable-length octet encoded uint and reads `u64` to rdr.
    fn read_var_uint(&mut self) -> Result<u64> {
        let contents = self.read_var_octet_string()?;
        Ok(BigEndian::read_uint(&contents, contents.len()))
    }
}

// Add this trait to all Readable things when this is used
impl<R: io::Read + ?Sized + Debug> ReadOerExt for R {}

pub trait WriteOerExt: Write + WriteBytesExt + Debug {
    #[inline]
    /// Encodes bytes as variable-length octet encoded string and writes to wtr.
    fn write_var_octet_string(&mut self, bytes: &[u8]) -> Result<()> {
        // Calculate the length of the variable-length octet string
        let length = bytes.len();
        // TODO: if length is zero, then err?
        if length <= 127 {
            self.write_u8(length as u8)?;
        } else {
            let bit_length_of_length = format!("{:b}", length).chars().count();
            let length_of_length = { bit_length_of_length as f32 / 8.0 }.ceil() as u8;
            self.write_u8(HIGH_BIT | length_of_length)?;
            self.write_uint::<BigEndian>(length as u64, length_of_length as usize)?;
        }
        self.write_all(&bytes)?;
        Ok(())
    }

    #[inline]
    /// Encodes `u64` as variable-length octet encoded unsigned integer and writes to wtr.
    fn write_var_uint(&mut self, uint: &u64) -> Result<()> {
        // Initialize the write buffer
        let mut bytes = [0; 8];
        // Write the u64 to the write buffer
        BigEndian::write_u64(&mut bytes, *uint);
        // Calculate the offset
        let offset = bytes.len() - bytesize(*uint);
        // Write the truncated write buffer as var_octet_string
        self.write_var_octet_string(&bytes[offset..bytes.len()])?;
        Ok(())
    }
}

// Add this trait to all Writable things when this is used
impl<W: io::Write + ?Sized + Debug> WriteOerExt for W {}

pub trait BufOerExt: Buf + Sized {
    #[inline]
    // TODO should this return a Bytes type or a Buf?
    /// Decodes variable-length octet string buffer to get `Bytes`.
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
    /// Decodes variable-length octet unsigned integer to get `u64`.
    fn get_var_uint(&mut self) -> u64 {
        let bytes = self.get_var_octet_string();
        let mut buf = bytes.into_buf();
        let size = buf.remaining();
        buf.get_uint_be(size)
    }
}

impl<B: Buf + Sized> BufOerExt for B {}

pub trait MutBufOerExt: BufMut + Sized {
    #[inline]
    /// Encodes bytes as variable-length octet encoded string and puts it into `Buf`.
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
    /// Encodes `u64` as variable-length octet encoded unsigned integer and puts it into `Buf`
    fn put_var_uint(&mut self, uint: &u64) {
        // Intitialize the put buffer
        let mut buf = vec![];
        // Write bytes to put buffer
        buf.put_u64_be(*uint);
        // Calculate the offset
        let offset = buf.len() - bytesize(*uint);
        // Put the truncated buffer as var_octet_string
        self.put_var_octet_string(&buf[offset..buf.len()].to_vec());
    }
}

impl<B: BufMut + Sized> MutBufOerExt for B {}

/// Helper function for determining actual number of bytes required to represent `u64`
fn bytesize(uint: u64) -> usize {
    let mut size: usize = 1;
    while size < 8 && uint >= (1u64 << size * 8) {
        size = size + 1;
    }
    return size;
}

#[cfg(test)]
mod reader_ext {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn it_reads_var_octet_strings() {
        // nothing test
        let nothing = vec![0];
        assert_eq!(
            Cursor::new(nothing).read_var_octet_string().unwrap().len(),
            0
        );

        // two byte variable-length octet string test
        let two_bytes = vec![0x01, 0xb0];
        assert_eq!(
            Cursor::new(two_bytes).read_var_octet_string().unwrap(),
            &[0xb0]
        );

        // larger byte variable-length octet string test
        let mut larger = vec![0x82, 0x01, 0x00];
        let mut larger_string: Vec<u8> = Vec::with_capacity(256 as usize);
        for _ in 0..256 {
            larger_string.push(0xb0);
        }
        larger.extend(&larger_string);
        assert_eq!(
            Cursor::new(larger).read_var_octet_string().unwrap(),
            &larger_string[..]
        );
    }

    #[test]
    #[should_panic]
    fn it_panics_on_reads_no_var_uint() {
        // FIXME: This should throw an error, but it is currently not implemented!
        let buf = vec![0x00];
        assert_eq!(Cursor::new(buf).read_var_uint().unwrap(), 0 as u64);
    }

    #[test]
    fn it_reads_one_byte_var_uint() {
        // one byte variable-length integer test
        let buf = vec![0x01, 0x09];
        assert_eq!(Cursor::new(buf).read_var_uint().unwrap(), 9 as u64);
    }

    #[test]
    fn it_reads_two_byte_var_uint() {
        // two byte variable-length integer test
        let buf = vec![0x02, 0x01, 0x02];
        assert_eq!(Cursor::new(buf).read_var_uint().unwrap(), 258 as u64);
    }

    #[test]
    fn it_reads_three_byte_var_uint() {
        // three byte variable-length integer test
        let buf = vec![0x03, 0x01, 0x02, 0x03];
        assert_eq!(Cursor::new(buf).read_var_uint().unwrap(), 66051 as u64);
    }

    #[test]
    fn it_reads_four_byte_var_uint() {
        // four byte variable-length integer test
        let buf = vec![0x04, 0x01, 0x02, 0x03, 0x04];
        assert_eq!(Cursor::new(buf).read_var_uint().unwrap(), 16909060 as u64);
    }

    #[test]
    fn it_reads_five_byte_var_uint() {
        // five byte variable-length integer test
        let buf = vec![0x05, 0x01, 0x02, 0x03, 0x04, 0x05];
        assert_eq!(Cursor::new(buf).read_var_uint().unwrap(), 4328719365 as u64);
    }

    #[test]
    fn it_reads_six_byte_var_uint() {
        // six byte variable-length integer test
        let buf = vec![0x06, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07];
        assert_eq!(
            Cursor::new(buf).read_var_uint().unwrap(),
            2211975595527 as u64
        );
    }

    #[test]
    fn it_reads_seven_byte_var_uint() {
        // seven byte variable-length integer test
        let buf = vec![0x07, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07];
        assert_eq!(
            Cursor::new(buf).read_var_uint().unwrap(),
            283686952306183 as u64
        );
    }

    #[test]
    fn it_reads_eight_byte_var_uint() {
        // eight byte variable-length integer test
        let buf = vec![0x08, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08];
        assert_eq!(
            Cursor::new(buf).read_var_uint().unwrap(),
            72623859790382856 as u64
        );
    }
}

#[cfg(test)]
mod writer_ext {
    use super::*;

    #[test]
    fn it_writes_empty_var_octet_strings() {
        let mut empty = vec![];
        empty.write_var_octet_string(&[]).unwrap();
        assert_eq!(empty, vec![0]);
    }

    #[test]
    fn it_writes_one_byte_var_octet_strings() {
        let mut one = vec![];
        one.write_var_octet_string(&[0xb0]).unwrap();
        assert_eq!(one, vec![0x01, 0xb0]);
    }

    #[test]
    fn it_writes_lots_of_bytes_var_octet_strings() {
        let mut larger = vec![];
        let mut larger_string: Vec<u8> = Vec::with_capacity(256 as usize);
        for _ in 0..256 {
            larger_string.push(0xb0);
        }
        larger.write_var_octet_string(&larger_string).unwrap();
        let mut expected = vec![0x82, 0x01, 0x00];
        expected.extend(larger_string);
        assert_eq!(larger.len(), 259);
        assert_eq!(larger, expected);
    }

    #[test]
    fn it_writes_zero_var_uint() {
        // zero value one byte variable-length integer test
        let mut wtr = Vec::new();
        let zero: u64 = 0;
        wtr.write_var_uint(&zero).unwrap();
        assert_eq!(wtr, vec![0x01, 0x00]);
    }

    #[test]
    fn it_writes_one_byte_var_uint() {
        // one byte variable-length integer test
        let mut wtr = Vec::new();
        let uint: u64 = 16;
        wtr.write_var_uint(&uint).unwrap();
        assert_eq!(wtr, vec![0x01, 0x10]);
    }

    #[test]
    fn it_writes_two_byte_var_uint() {
        // two byte variable-length integer test
        let mut wtr = Vec::new();
        let uint: u64 = 259;
        wtr.write_var_uint(&uint).unwrap();
        assert_eq!(wtr, vec![0x02, 0x01, 0x03]);
    }
    #[test]
    fn it_writes_three_byte_var_uint() {
        // four byte variable-length integer test
        let mut wtr = Vec::new();
        let uint: u64 = 0x01020305;
        wtr.write_var_uint(&uint).unwrap();
        assert_eq!(wtr, vec![0x04, 0x01, 0x02, 0x03, 0x05]);
    }
}

#[cfg(test)]
mod bufoer_ext {
    use super::*;
    use bytes::Bytes;
    use std::io::Cursor;

    #[test]
    fn it_gets_var_octet_strings() {
        // nothing test
        let nothing = vec![0];
        assert_eq!(Cursor::new(nothing).get_var_octet_string().len(), 0);

        // two byte variable-length octet string test
        let two_bytes: Vec<u8> = vec![0x01, 0xb0];
        assert_eq!(
            Cursor::new(two_bytes).get_var_octet_string(),
            Bytes::from(&b"\xb0"[..])
        );

        // larger byte variable-length octet string test
        let mut larger = vec![0x82, 0x01, 0x00];
        let mut larger_string: Vec<u8> = Vec::with_capacity(256 as usize);
        for _ in 0..256 {
            larger_string.push(0xb0);
        }
        larger.extend(&larger_string);
        assert_eq!(
            Cursor::new(larger).get_var_octet_string(),
            &larger_string[..]
        );
    }

    #[test]
    #[should_panic]
    // TODO: This test should fail... but in what way? (No Result<T, E>)
    fn it_panics_on_gets_no_var_uint() {
        // byte variable-length integer test
        let buf = vec![0x00];
        assert_eq!(Cursor::new(buf).get_var_uint(), 0 as u64);
    }

    #[test]
    fn it_gets_one_byte_var_uint() {
        // one byte variable-length integer test
        let buf = vec![0x01, 0x09];
        assert_eq!(Cursor::new(buf).get_var_uint(), 9 as u64);
    }

    #[test]
    fn it_gets_two_byte_var_uint() {
        // two byte variable-length integer test
        let buf = vec![0x02, 0x01, 0x02];
        assert_eq!(Cursor::new(buf).get_var_uint(), 258 as u64);
    }

    #[test]
    fn it_gets_three_byte_var_uint() {
        // three byte variable-length integer test
        let buf = vec![0x03, 0x01, 0x02, 0x03];
        assert_eq!(Cursor::new(buf).get_var_uint(), 66051 as u64);
    }

    #[test]
    fn it_gets_four_byte_var_uint() {
        // four byte variable-length integer test
        let buf = vec![0x04, 0x01, 0x02, 0x03, 0x04];
        assert_eq!(Cursor::new(buf).get_var_uint(), 16909060 as u64);
    }

    #[test]
    fn it_gets_five_byte_var_uint() {
        // five byte variable-length integer test
        let buf = vec![0x05, 0x01, 0x02, 0x03, 0x04, 0x05];
        assert_eq!(Cursor::new(buf).get_var_uint(), 4328719365 as u64);
    }

    #[test]
    fn it_gets_six_byte_var_uint() {
        // six byte variable-length integer test
        let buf = vec![0x06, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07];
        assert_eq!(Cursor::new(buf).get_var_uint(), 2211975595527 as u64);
    }

    #[test]
    fn it_gets_seven_byte_var_uint() {
        // seven byte variable-length integer test
        let buf = vec![0x07, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07];
        assert_eq!(Cursor::new(buf).get_var_uint(), 283686952306183 as u64);
    }

    #[test]
    fn it_gets_eight_byte_var_uint() {
        // eight byte variable-length integer test
        let buf = vec![0x08, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08];
        assert_eq!(Cursor::new(buf).get_var_uint(), 72623859790382856 as u64);
    }
}

#[cfg(test)]
mod bufmutoer_ext {
    use super::*;

    #[test]
    fn it_puts_empty_var_octet_strings() {
        let mut buf = vec![];
        let empty = vec![];
        buf.put_var_octet_string(Bytes::from(empty));
        assert_eq!(buf, vec![0]);
    }

    #[test]
    fn it_puts_one_byte_var_octet_strings() {
        let mut buf = vec![];
        let one = vec![176u8];
        buf.put_var_octet_string(one);
        assert_eq!(buf, vec![0x01, 0xb0]);
    }

    #[test]
    fn it_puts_lots_of_bytes_var_octet_strings() {
        let mut larger = vec![];
        let mut larger_string: Vec<u8> = Vec::with_capacity(256 as usize);
        for _ in 0..256 {
            larger_string.push(0xb0);
        }
        larger.put_var_octet_string(&larger_string);
        let mut expected = vec![0x82, 0x01, 0x00];
        expected.extend(larger_string);
        assert_eq!(larger.len(), 259);
        assert_eq!(larger, expected);
    }

    #[test]
    fn it_puts_zero_var_uint() {
        // zero value one byte variable-length integer test
        let mut buf = Vec::new();
        let zero: u64 = 0;
        buf.put_var_uint(&zero);
        assert_eq!(buf, vec![0x01, 0x00]);
    }

    #[test]
    fn it_puts_one_byte_var_uint() {
        // one byte variable-length integer test
        let mut buf = Vec::new();
        let uint: u64 = 16;
        buf.put_var_uint(&uint);
        assert_eq!(buf, vec![0x01, 0x10]);
    }

    #[test]
    fn it_puts_two_byte_var_uint() {
        // two byte variable-length integer test
        let mut buf = Vec::new();
        let uint: u64 = 259;
        buf.put_var_uint(&uint);
        assert_eq!(buf, vec![0x02, 0x01, 0x03]);
    }

    #[test]
    fn it_puts_three_byte_var_uint() {
        // four byte variable-length integer test
        let mut buf = Vec::new();
        let uint: u64 = 0x01020305;
        buf.put_var_uint(&uint);
        assert_eq!(buf, vec![0x04, 0x01, 0x02, 0x03, 0x05]);
    }
}
