use std::io::Error;
use byteorder::{BigEndian, WriteBytesExt};

const MSB: u8 = 0x80;

// TODO maybe just further extend the byteorder traits and export that as OER, since it implements
// all the functions except for the var octet strings

pub fn write_var_octet_string(data: &mut Vec<u8>, string: &Vec<u8>) -> Result<(), Error> {
    let length = string.len();

    if length < 127 {
        data.write_u8(length as u8)?;
    } else {
        let bit_length_of_length = format!("{:b}", length).chars().count();
        let length_of_length = {bit_length_of_length as f32 / 8.0}.ceil() as u8;
        data.write_u8(MSB | length_of_length)?;
        data.write_uint::<BigEndian>(length as u64, length_of_length as usize)?;
    }
    data.extend(string);
    Ok(())
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
}
