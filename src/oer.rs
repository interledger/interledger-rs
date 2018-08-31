use byteorder::{BigEndian, ReadBytesExt, WriteBytesExt};
use std::fmt::Debug;
use std::io::{self, Cursor, Read, Result, Write, Error, ErrorKind};
use num_bigint::BigUint;

const HIGH_BIT: u8 = 0x80;
const LOWER_SEVEN_BITS: u8 = 0x7f;

// TODO don't copy the code over into the functions for the trait

// TODO test traits
pub trait ReadOerExt: Read + ReadBytesExt + Debug {
    #[inline]
    // TODO should this function return a vec or a slice?
    fn read_var_octet_string(&mut self) -> Result<Vec<u8>> {
        let length: u8 = self.read_u8()?;

        if length == 0 {
            return Ok(vec![]);
        }

        let actual_length: u64 = if length & HIGH_BIT != 0 {
            let length_prefix_length = length & LOWER_SEVEN_BITS;
            // TODO check for canonical length
            self.read_uint::<BigEndian>(length_prefix_length as usize)? as u64
        } else {
            length as u64
        };

        // TODO handle if the length is too long
        // TODO don't copy this twice
        let mut buf = Vec::with_capacity(actual_length as usize);
        self.take(actual_length).read_to_end(&mut buf)?;
        Ok(buf)
    }

    #[inline]
    fn read_var_uint(&mut self) -> Result<BigUint> {
        let mut contents = self.read_var_octet_string()?;
        Ok(BigUint::from_bytes_be(&contents))
    }
}

// Add this trait to all Readable things when this is used
impl<R: io::Read + ?Sized + Debug> ReadOerExt for R {}

pub trait WriteOerExt: Write + WriteBytesExt {
    #[inline]
    fn write_var_octet_string(&mut self, string: &[u8]) -> Result<()> {
        let length = string.len();

        if length < 127 {
            self.write_u8(length as u8)?;
        } else {
            let bit_length_of_length = format!("{:b}", length).chars().count();
            let length_of_length = { bit_length_of_length as f32 / 8.0 }.ceil() as u8;
            self.write_u8(HIGH_BIT | length_of_length)?;
            self.write_uint::<BigEndian>(length as u64, length_of_length as usize)?;
        }
        self.write_all(string)?;
        Ok(())
    }

    #[inline]
    // Write a u64 as an OER VarUInt
    fn write_var_uint(&mut self, uint: BigUint) -> Result<()> {
        self.write_var_octet_string(&uint.to_bytes_be())?;
        Ok(())
    }
}

// Add this trait to all Writable things when this is used
impl<W: io::Write + ?Sized> WriteOerExt for W {}

pub fn write_var_octet_string<T: Write + WriteBytesExt>(data: &mut T, string: &[u8]) -> Result<()> {
    let length = string.len();

    if length < 127 {
        data.write_u8(length as u8)?;
    } else {
        let bit_length_of_length = format!("{:b}", length).chars().count();
        let length_of_length = { bit_length_of_length as f32 / 8.0 }.ceil() as u8;
        data.write_u8(HIGH_BIT | length_of_length)?;
        data.write_uint::<BigEndian>(length as u64, length_of_length as usize)?;
    }
    data.write_all(string)?;
    Ok(())
}

// TODO make this so it doesn't have to be a u8 slice
pub fn read_var_octet_string(data: &[u8]) -> Result<&[u8]> {
    let mut reader = Cursor::new(data);
    let length: u8 = reader.read_u8()?;

    if length == 0 {
        let empty: [u8; 0] = [];
        return Ok(&empty.to_owned());
    }

    let actual_length: usize = if length & HIGH_BIT != 0 {
        let length_prefix_length = length & LOWER_SEVEN_BITS;
        reader.read_uint::<BigEndian>(length_prefix_length as usize)? as usize
    // TODO check for canonical length
    } else {
        length as usize
    };

    let pos = reader.position() as usize;

    // TODO handle if the length is too long
    Ok(&data[pos..(pos + actual_length)])
}

#[cfg(test)]
mod standalone {
    use super::*;

    #[test]
    fn it_writes_var_octet_strings() {
        let mut empty = vec![];
        write_var_octet_string(&mut empty, &vec![]).unwrap();
        assert_eq!(empty, vec![0]);

        let mut one = vec![];
        write_var_octet_string(&mut one, &vec![0xb0]).unwrap();
        assert_eq!(one, vec![0x01, 0xb0]);

        let mut larger = vec![];
        let mut larger_string: Vec<u8> = Vec::with_capacity(256 as usize);
        for _ in 0..256 {
            larger_string.push(0xb0);
        }
        write_var_octet_string(&mut larger, &larger_string).unwrap();
        let mut expected = vec![0x82, 0x01, 0x00];
        expected.extend(larger_string);
        assert_eq!(larger.len(), 259);
        assert_eq!(larger, expected);
    }

    #[test]
    fn it_reads_var_octet_strings() {
        let empty: [u8; 0] = [];
        assert_eq!(read_var_octet_string(&vec![0]).unwrap(), &empty);

        assert_eq!(read_var_octet_string(&vec![0x01, 0xb0]).unwrap(), &[0xb0]);

        let mut larger = vec![0x82, 0x01, 0x00];
        let mut larger_string: Vec<u8> = Vec::with_capacity(256 as usize);
        for _ in 0..256 {
            larger_string.push(0xb0);
        }
        larger.extend(&larger_string);
        assert_eq!(read_var_octet_string(&larger).unwrap(), &larger_string[..]);
    }
}
