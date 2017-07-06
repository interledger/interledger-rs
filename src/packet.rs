use oer;
use std::io::{Cursor, Error, ErrorKind};
use byteorder::{BigEndian, WriteBytesExt, ReadBytesExt};

#[derive(Debug, PartialEq)]
#[repr(u8)]
enum PacketType {
    IlpPayment = 1,
    Unknown
}
impl From<u8> for PacketType {
    fn from(type_int: u8) -> Self {
        match type_int {
            1 => PacketType::IlpPayment,
            _ => PacketType::Unknown
        }

    }

}

pub enum IlpPacket {
    IlpPayment,
}
// TODO add IlpPacket trait with serialization functions

// TODO add JSON serialization for debugging and CLI
#[derive(Debug, PartialEq, Clone)]
pub struct IlpPayment {
    amount: u64,
    account: String,
    data: Vec<u8>
}

impl IlpPayment {
    pub fn from_bytes(bytes: &[u8]) -> Result<IlpPayment, Error> {
        let (packet_type, contents) = deserialize_envelope(bytes)?;
        if packet_type != PacketType::IlpPayment {
            return Err(Error::new(ErrorKind::Other, format!("attempted to deserialize packet type {:?} as IlpPayment", packet_type)));
        }

        let mut reader = Cursor::new(contents);
        let amount = reader.read_u64::<BigEndian>()?;
        let account_bytes = oer::read_var_octet_string(&contents[reader.position() as usize..])?;
        let account = String::from_utf8(account_bytes.to_vec()).map_err(|_| Error::new(ErrorKind::Other, "account not utf8"))?;
        let start_of_data_pos = reader.position() + account.len() as u64 + 1;
        let data_bytes = oer::read_var_octet_string(&contents[start_of_data_pos as usize..])?;
        let data = data_bytes.to_vec();
        Ok(IlpPayment {
            amount: amount,
            account: account,
            data: data,
        })
    }

    pub fn to_bytes(&self) -> Result<Vec<u8>, Error> {
        let mut payment_vec: Vec<u8> = Vec::new();
        payment_vec.write_u64::<BigEndian>(self.amount)?;
        oer::write_var_octet_string(&mut payment_vec, &self.account.to_string().into_bytes())?;
        oer::write_var_octet_string(&mut payment_vec, &self.data)?;
        payment_vec.write_u8(0)?; // extensibility
        serialize_envelope(PacketType::IlpPayment, &payment_vec)
    }

}

fn serialize_envelope(packet_type: PacketType, contents: &Vec<u8>) -> Result<Vec<u8>, Error> {
    // TODO do this mutably so we don't need to copy the data
    let mut packet = Vec::new();
    packet.write_u8(packet_type as u8)?;
    oer::write_var_octet_string(&mut packet, contents)?;
    Ok(packet)
}

fn deserialize_envelope(bytes: &[u8]) -> Result<(PacketType, &[u8]), Error> {
    let mut reader = Cursor::new(bytes);
    let packet_type = PacketType::from(reader.read_u8()?);
    let pos = reader.position() as usize;
    let slice: &[u8] = &reader.get_ref()[pos..];
    Ok((packet_type,
       oer::read_var_octet_string(slice)?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64;

    fn hex_to_bytes(hex: &str) -> Vec<u8> {
        let mut chars = hex.chars();
        let mut bytes = Vec::new();
        for _i in 0..(hex.len() / 2) {
            let first: u8 = chars.next().unwrap().to_digit(16).unwrap() as u8;
            let second: u8 = chars.next().unwrap().to_digit(16).unwrap() as u8;
            let byte = (first << 4) | second;
            bytes.push(byte)
        }
        bytes
    }

    #[test]
    fn it_serializes_ilp_payments() {
        let without_data = IlpPayment {
            amount: 107,
            account: "example.alice".to_string(),
            data: vec![]
        };
        assert_eq!(
            without_data.to_bytes().unwrap(),
            hex_to_bytes("0118000000000000006b0d6578616d706c652e616c6963650000"),
            "without data");

        let with_data = IlpPayment {
            amount: 100,
            account: "example.bob".to_string(),
            data: base64::decode("ZZZZ").unwrap()
        };
        assert_eq!(
            with_data.to_bytes().unwrap(),
            hex_to_bytes("011900000000000000640b6578616d706c652e626f620365965900"),
            "with data");
    }

    #[test]
    fn it_deserializes_ilp_payments() {
        let without_data = IlpPayment {
            amount: 107,
            account: "example.alice".to_string(),
            data: vec![]
        };
        assert_eq!(
            IlpPayment::from_bytes(&hex_to_bytes("0118000000000000006b0d6578616d706c652e616c6963650000")[..]).unwrap(),
            without_data,
            "without data");

        let with_data = IlpPayment {
            amount: 100,
            account: "example.bob".to_string(),
            data: base64::decode("ZZZZ").unwrap()
        };
        assert_eq!(
            IlpPayment::from_bytes(&hex_to_bytes("011900000000000000640b6578616d706c652e626f620365965900")[..]).unwrap(),
            with_data,
            "with data");
    }
}

