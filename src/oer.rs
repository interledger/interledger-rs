use std::io::{Cursor, Error};
use byteorder::{BigEndian, WriteBytesExt, ReadBytesExt};

const HIGH_BIT: u8 = 0x80;
const LOWER_SEVEN_BITS: u8 = 0x7f;

// TODO maybe just further extend the byteorder traits and export that as OER, since it implements
// all the functions except for the var octet strings

pub fn write_var_octet_string(data: &mut Vec<u8>, string: &Vec<u8>) -> Result<(), Error> {
    let length = string.len();

    if length < 127 {
        data.write_u8(length as u8)?;
    } else {
        let bit_length_of_length = format!("{:b}", length).chars().count();
        let length_of_length = {
            bit_length_of_length as f32 / 8.0
        }.ceil() as u8;
        data.write_u8(HIGH_BIT | length_of_length)?;
        data.write_uint::<BigEndian>(
            length as u64,
            length_of_length as usize,
        )?;
    }
    data.extend(string);
    Ok(())
}

pub fn read_var_octet_string(data: &[u8]) -> Result<&[u8], Error> {
    let mut reader = Cursor::new(data);
    let length: u8 = reader.read_u8()?;

    if length == 0 {
        let empty: [u8; 0] = [];
        return Ok(&empty.to_owned());
    }

    let actual_length: usize = if length & HIGH_BIT != 0 {
        println!("got here");
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
mod tests {
    use super::*;

    #[test]
    fn it_writes_octet_strings() {
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
    fn it_reads_octet_string() {
        assert_eq!(read_var_octet_string(&vec![0]).unwrap(), &[]);

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
