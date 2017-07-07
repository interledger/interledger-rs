use oer;
use errors::ParseError;
use std::io::{Cursor};
use byteorder::{BigEndian, WriteBytesExt, ReadBytesExt};
use serde_json;
use base64;

#[derive(Debug, PartialEq)]
#[repr(u8)]
enum PacketType {
    IlpPayment = 1,
    Unknown,
}
impl From<u8> for PacketType {
    fn from(type_int: u8) -> Self {
        match type_int {
            1 => PacketType::IlpPayment,
            _ => PacketType::Unknown,
        }

    }
}

pub enum IlpPacket {
    IlpPayment,
}
// TODO add IlpPacket trait with serialization functions

#[derive(Debug, PartialEq, Clone)]
pub struct IlpPayment {
    amount: u64,
    account: String,
    data: Vec<u8>,
}

// TODO there must be a better way of changing data to base64
#[derive(Serialize, Deserialize)]
struct IlpPaymentForJson {
    amount: u64,
    account: String,
    data: String
}

impl IlpPayment {
    pub fn from_bytes(bytes: &[u8]) -> Result<IlpPayment, ParseError> {
        let (packet_type, contents) = deserialize_envelope(bytes)?;
        if packet_type != PacketType::IlpPayment {
            return Err(ParseError::WrongType("attempted to deserialize other packet type as IlpPayment"));
        }

        let mut reader = Cursor::new(contents);
        let amount = reader.read_u64::<BigEndian>()?;
        let account_bytes = oer::read_var_octet_string(&contents[reader.position() as usize..])?;
        let account = String::from_utf8(account_bytes.to_vec()).map_err(|_| {
            ParseError::InvalidPacket("account not utf8")
        })?;
        let start_of_data_pos = reader.position() + account.len() as u64 + 1;
        let data_bytes = oer::read_var_octet_string(&contents[start_of_data_pos as usize..])?;
        let data = data_bytes.to_vec();
        Ok(IlpPayment {
            amount: amount,
            account: account,
            data: data,
        })
    }

    pub fn to_bytes(&self) -> Result<Vec<u8>, ParseError> {
        let mut payment_vec: Vec<u8> = Vec::new();
        payment_vec.write_u64::<BigEndian>(self.amount)?;
        oer::write_var_octet_string(&mut payment_vec, &self.account.to_string().into_bytes())?;
        oer::write_var_octet_string(&mut payment_vec, &self.data)?;
        payment_vec.write_u8(0)?; // extensibility
        serialize_envelope(PacketType::IlpPayment, &payment_vec)
    }

    pub fn to_json_string(&self) -> Result<String, ParseError> {
        let data_string = base64::encode_config(&self.data, base64::URL_SAFE_NO_PAD);
        let for_json = IlpPaymentForJson {
            amount: self.amount,
            account: self.account.to_string(),
            data: data_string
        };
        serde_json::to_string(&for_json).map_err(|err| ParseError::Json(err))
    }

    pub fn from_json_str(string: &str) -> Result<IlpPayment, ParseError> {
        let json_rep: IlpPaymentForJson = serde_json::from_str(string)
            .map_err(|err| ParseError::Json(err))?;
        let data = base64::decode_config(&json_rep.data, base64::URL_SAFE_NO_PAD)
            .map_err(|err| ParseError::Base64(err))?;
        Ok(IlpPayment {
            amount: json_rep.amount,
            account: json_rep.account.to_string(),
            data: data
        })
    }
}

fn serialize_envelope(packet_type: PacketType, contents: &Vec<u8>) -> Result<Vec<u8>, ParseError> {
    // TODO do this mutably so we don't need to copy the data
    let mut packet = Vec::new();
    packet.write_u8(packet_type as u8)?;
    oer::write_var_octet_string(&mut packet, contents)?;
    Ok(packet)
}

fn deserialize_envelope(bytes: &[u8]) -> Result<(PacketType, &[u8]), ParseError> {
    let mut reader = Cursor::new(bytes);
    let packet_type = PacketType::from(reader.read_u8()?);
    let pos = reader.position() as usize;
    let slice: &[u8] = &reader.get_ref()[pos..];
    Ok((packet_type, oer::read_var_octet_string(slice)?))
}

#[cfg(test)]
mod tests {
    use super::*;

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
            data: vec![],
        };
        assert_eq!(
            without_data.to_bytes().unwrap(),
            hex_to_bytes("0118000000000000006b0d6578616d706c652e616c6963650000"),
            "without data"
        );

        let with_data = IlpPayment {
            amount: 100,
            account: "example.bob".to_string(),
            data: base64::decode("ZZZZ").unwrap(),
        };
        assert_eq!(
            with_data.to_bytes().unwrap(),
            hex_to_bytes("011900000000000000640b6578616d706c652e626f620365965900"),
            "with data"
        );
    }

    #[test]
    fn it_deserializes_ilp_payments() {
        let without_data = IlpPayment {
            amount: 107,
            account: "example.alice".to_string(),
            data: vec![],
        };
        assert_eq!(
            IlpPayment::from_bytes(
                &hex_to_bytes("0118000000000000006b0d6578616d706c652e616c6963650000")[..],
            ).unwrap(),
            without_data,
            "without data"
        );

        let with_data = IlpPayment {
            amount: 100,
            account: "example.bob".to_string(),
            data: base64::decode("ZZZZ").unwrap(),
        };
        assert_eq!(
            IlpPayment::from_bytes(
                &hex_to_bytes("011900000000000000640b6578616d706c652e626f620365965900")[..],
            ).unwrap(),
            with_data,
            "with data"
        );
    }

    #[test]
    fn it_encodes_ilp_payments_as_json() {
        let without_data = IlpPayment {
            amount: 107,
            account: "example.alice".to_string(),
            data: vec![],
        };
        assert_eq!(without_data.to_json_string().unwrap(), "{\"amount\":107,\"account\":\"example.alice\",\"data\":\"\"}");

        let with_data = IlpPayment {
            amount: 100,
            account: "example.bob".to_string(),
            data: base64::decode("ZZZZ").unwrap(),
        };
        assert_eq!(with_data.to_json_string().unwrap(), "{\"amount\":100,\"account\":\"example.bob\",\"data\":\"ZZZZ\"}");
    }

    #[test]
    fn it_decodes_ilp_payments_as_json() {
        let without_data = IlpPayment {
            amount: 107,
            account: "example.alice".to_string(),
            data: vec![],
        };
        assert_eq!(IlpPayment::from_json_str("{\"amount\":107,\"account\":\"example.alice\",\"data\":\"\"}").unwrap(), without_data);

        let with_data = IlpPayment {
            amount: 100,
            account: "example.bob".to_string(),
            data: base64::decode("ZZZZ").unwrap(),
        };
        assert_eq!(IlpPayment::from_json_str("{\"amount\":100,\"account\":\"example.bob\",\"data\":\"ZZZZ\"}").unwrap(), with_data);
    }
}
