use oer;
use std::io::Error;
use byteorder::{BigEndian, WriteBytesExt};

#[repr(u8)]
enum PacketType {
    IlpPayment = 1,
}

pub enum IlpPacket {
    IlpPayment,
}
// TODO add IlpPacket trait with serialization functions

// TODO add JSON serialization for debugging and CLI
pub struct IlpPayment {
    amount: u64,
    account: String,
    data: Vec<u8>
}

impl IlpPayment {
    // TODO add validation

    pub fn serialize(&self) -> Result<Vec<u8>, Error> {
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
            without_data.serialize().unwrap(),
            hex_to_bytes("0118000000000000006b0d6578616d706c652e616c6963650000"),
            "without data");

        let with_data = IlpPayment {
            amount: 100,
            account: "example.bob".to_string(),
            data: base64::decode("ZZZZ").unwrap()
        };
        assert_eq!(
            with_data.serialize().unwrap(),
            hex_to_bytes("011900000000000000640b6578616d706c652e626f620365965900"),
            "with data");
    }
}

