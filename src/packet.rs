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
    IlqpLiquidityRequest = 2,
    IlqpLiquidityResponse = 3,
    IlqpBySourceRequest = 4,
    IlqpBySourceResponse = 5,
    IlqpByDestinationRequest = 6,
    IlqpByDestinationResponse = 7,
    IlpError = 8,
    Unknown,
}
impl From<u8> for PacketType {
    fn from(type_int: u8) -> Self {
        match type_int {
            1 => PacketType::IlpPayment,
            2 => PacketType::IlqpLiquidityRequest,
            3 => PacketType::IlqpLiquidityResponse,
            4 => PacketType::IlqpBySourceRequest,
            5 => PacketType::IlqpBySourceResponse,
            6 => PacketType::IlqpByDestinationRequest,
            7 => PacketType::IlqpByDestinationResponse,
            8 => PacketType::IlpError,
            _ => PacketType::Unknown,
        }

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

pub enum IlpPacket {
    IlpPayment,
    IlqpLiquidityRequest,
    IlqpLiquidityResponse,
    IlqpBySourceRequest,
    IlqpBySourceResponse,
    IlqpByDestinationRequest,
    IlqpByDestinationResponse,
    IlpError,
}
// TODO add IlpPacket trait with serialization functions

#[derive(Debug, PartialEq, Clone)]
pub struct IlpPayment {
    pub amount: u64,
    pub account: String,
    pub data: Vec<u8>,
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
        let mut bytes: Vec<u8> = Vec::new();
        bytes.write_u64::<BigEndian>(self.amount)?;
        oer::write_var_octet_string(&mut bytes, &self.account.to_string().into_bytes())?;
        oer::write_var_octet_string(&mut bytes, &self.data)?;
        bytes.write_u8(0)?; // extensibility
        serialize_envelope(PacketType::IlpPayment, &bytes)
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

#[derive(Debug, PartialEq, Clone, Serialize, Deserialize)]
pub struct IlqpBySourceRequest {
    pub destination_account: String,
    pub source_amount: u64,
    pub destination_hold_duration: u32,
}
impl IlqpBySourceRequest {
    pub fn from_bytes(bytes: &[u8]) -> Result<IlqpBySourceRequest, ParseError> {
        let (packet_type, contents) = deserialize_envelope(bytes)?;
        if packet_type != PacketType::IlqpBySourceRequest {
            return Err(ParseError::WrongType("attempted to deserialize other packet type as IlqpBySourceRequest"));
        }

        let mut reader = Cursor::new(contents);
        let destination_account_bytes = oer::read_var_octet_string(&contents[reader.position() as usize..])?;
        let destination_account = String::from_utf8(destination_account_bytes.to_vec()).map_err(|_| {
            ParseError::InvalidPacket("account not utf8")
        })?;
        reader.set_position((destination_account_bytes.len() + 1) as u64);
        let source_amount = reader.read_u64::<BigEndian>()?;
        let destination_hold_duration = reader.read_u32::<BigEndian>()?;
        Ok(IlqpBySourceRequest {
            destination_account: destination_account,
            source_amount: source_amount,
            destination_hold_duration: destination_hold_duration
        })
    }

    pub fn to_bytes(&self) -> Result<Vec<u8>, ParseError> {
        let mut bytes: Vec<u8> = Vec::new();
        oer::write_var_octet_string(&mut bytes, &self.destination_account.to_string().into_bytes())?;
        bytes.write_u64::<BigEndian>(self.source_amount)?;
        bytes.write_u32::<BigEndian>(self.destination_hold_duration)?;
        bytes.write_u8(0)?; // extensibility
        serialize_envelope(PacketType::IlqpBySourceRequest, &bytes)
    }

    pub fn to_json_string(&self) -> Result<String, ParseError> {
        serde_json::to_string(&self).map_err(|err| ParseError::Json(err))
    }

    pub fn from_json_str(string: &str) -> Result<IlqpBySourceRequest, ParseError> {
        let result: IlqpBySourceRequest = serde_json::from_str(string)
            .map_err(|err| ParseError::Json(err))?;
        Ok(result)
    }
}

#[derive(Debug, PartialEq, Clone, Serialize, Deserialize)]
pub struct IlqpByDestinationRequest {
    pub destination_account: String,
    pub destination_amount: u64,
    pub destination_hold_duration: u32,
}
impl IlqpByDestinationRequest {
    pub fn from_bytes(bytes: &[u8]) -> Result<IlqpByDestinationRequest, ParseError> {
        let (packet_type, contents) = deserialize_envelope(bytes)?;
        if packet_type != PacketType::IlqpByDestinationRequest {
            return Err(ParseError::WrongType("attempted to deserialize other packet type as IlqpByDestinationRequest"));
        }

        let mut reader = Cursor::new(contents);
        let destination_account_bytes = oer::read_var_octet_string(&contents[reader.position() as usize..])?;
        let destination_account = String::from_utf8(destination_account_bytes.to_vec()).map_err(|_| {
            ParseError::InvalidPacket("account not utf8")
        })?;
        reader.set_position((destination_account_bytes.len() + 1) as u64);
        let destination_amount = reader.read_u64::<BigEndian>()?;
        let destination_hold_duration = reader.read_u32::<BigEndian>()?;
        Ok(IlqpByDestinationRequest {
            destination_account: destination_account,
            destination_amount: destination_amount,
            destination_hold_duration: destination_hold_duration
        })
    }

    pub fn to_bytes(&self) -> Result<Vec<u8>, ParseError> {
        let mut bytes: Vec<u8> = Vec::new();
        oer::write_var_octet_string(&mut bytes, &self.destination_account.to_string().into_bytes())?;
        bytes.write_u64::<BigEndian>(self.destination_amount)?;
        bytes.write_u32::<BigEndian>(self.destination_hold_duration)?;
        bytes.write_u8(0)?; // extensibility
        serialize_envelope(PacketType::IlqpByDestinationRequest, &bytes)
    }

    pub fn to_json_string(&self) -> Result<String, ParseError> {
        serde_json::to_string(&self).map_err(|err| ParseError::Json(err))
    }

    pub fn from_json_str(string: &str) -> Result<IlqpByDestinationRequest, ParseError> {
        let result: IlqpByDestinationRequest = serde_json::from_str(string)
            .map_err(|err| ParseError::Json(err))?;
        Ok(result)
    }
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

    #[cfg(test)]
    mod ilp_payment {
        use super::*;

        #[test]
        fn serialize() {
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
        fn deserialize() {
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
        fn encode_as_json() {
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
        fn decode_as_json() {
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


    #[cfg(test)]
    mod ilqp_by_source_request {
        use super::*;

        #[test]
        fn serialize() {
            let request = IlqpBySourceRequest {
                source_amount: 9000000000,
                destination_account: "example.alice".to_string(),
                destination_hold_duration: 3000
            };
            assert_eq!(request.to_bytes().unwrap(),
                hex_to_bytes("041b0d6578616d706c652e616c6963650000000218711a0000000bb800"))

        }

        #[test]
        fn deserialize() {
            let request = IlqpBySourceRequest {
                source_amount: 9000000000,
                destination_account: "example.alice".to_string(),
                destination_hold_duration: 3000
            };

            assert_eq!(IlqpBySourceRequest::from_bytes(&hex_to_bytes("041b0d6578616d706c652e616c6963650000000218711a0000000bb800")).unwrap(),
            request)
        }
    }

    #[cfg(test)]
    mod ilqp_by_destination_request {
        use super::*;

        #[test]
        fn serialize() {
            let request = IlqpByDestinationRequest {
                destination_amount: 9000000000,
                destination_account: "example.alice".to_string(),
                destination_hold_duration: 3000
            };
            assert_eq!(request.to_bytes().unwrap(),
                hex_to_bytes("061b0d6578616d706c652e616c6963650000000218711a0000000bb800"))

        }

        #[test]
        fn deserialize() {
            let request = IlqpByDestinationRequest {
                destination_amount: 9000000000,
                destination_account: "example.alice".to_string(),
                destination_hold_duration: 3000
            };

            assert_eq!(IlqpByDestinationRequest::from_bytes(&hex_to_bytes("061b0d6578616d706c652e616c6963650000000218711a0000000bb800")).unwrap(),
            request)
        }
    }
}
