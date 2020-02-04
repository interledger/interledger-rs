use super::errors::ParseError;
use super::oer::{MutBufOerExt, ReadOerExt};
use byteorder::{BigEndian, ReadBytesExt};
use bytes::BufMut;
use chrono::{DateTime, TimeZone, Utc};
use num_bigint::BigUint;
#[cfg(test)]
use once_cell::sync::Lazy;
use std::io::prelude::*;
use std::io::Cursor;
use std::ops::Add;
use std::str;

static GENERALIZED_TIME_FORMAT: &str = "%Y%m%d%H%M%S%.3fZ";

pub trait Serializable<T> {
    fn from_bytes(bytes: &[u8]) -> Result<T, ParseError>;

    fn to_bytes(&self) -> Vec<u8>;
}

#[derive(Debug, PartialEq, Clone)]
#[repr(u8)]
enum PacketType {
    Message = 6,
    Response = 1,
    Error = 2,
    Unknown,
}
impl From<u8> for PacketType {
    fn from(type_int: u8) -> Self {
        match type_int {
            6 => PacketType::Message,
            1 => PacketType::Response,
            2 => PacketType::Error,
            _ => PacketType::Unknown,
        }
    }
}

#[derive(Debug, PartialEq, Clone)]
pub enum BtpPacket {
    Message(BtpMessage),
    Response(BtpResponse),
    Error(BtpError),
}

impl Serializable<BtpPacket> for BtpPacket {
    fn from_bytes(bytes: &[u8]) -> Result<BtpPacket, ParseError> {
        match PacketType::from(bytes[0]) {
            PacketType::Message => Ok(BtpPacket::Message(BtpMessage::from_bytes(bytes)?)),
            PacketType::Response => Ok(BtpPacket::Response(BtpResponse::from_bytes(bytes)?)),
            PacketType::Error => Ok(BtpPacket::Error(BtpError::from_bytes(bytes)?)),
            PacketType::Unknown => Err(ParseError::InvalidPacket(format!(
                "Unknown packet type: {}",
                bytes[0]
            ))),
        }
    }

    fn to_bytes(&self) -> Vec<u8> {
        match self {
            BtpPacket::Message(packet) => packet.to_bytes(),
            BtpPacket::Response(packet) => packet.to_bytes(),
            BtpPacket::Error(packet) => packet.to_bytes(),
        }
    }
}

#[derive(Debug, PartialEq, Clone)]
pub enum ContentType {
    ApplicationOctetStream = 0,
    TextPlainUtf8 = 1,
    Unknown,
}
impl From<u8> for ContentType {
    fn from(type_int: u8) -> Self {
        match type_int {
            0 => ContentType::ApplicationOctetStream,
            1 => ContentType::TextPlainUtf8,
            _ => ContentType::Unknown,
        }
    }
}

#[derive(Debug, PartialEq, Clone)]
pub struct ProtocolData {
    pub protocol_name: String,
    pub content_type: ContentType,
    pub data: Vec<u8>,
}
fn read_protocol_data<T>(reader: &mut T) -> Result<Vec<ProtocolData>, ParseError>
where
    T: ReadOerExt,
{
    let mut protocol_data = Vec::new();

    let num_entries = reader.read_var_uint()?;
    let mut i = BigUint::from(0 as u32);
    while i < num_entries {
        i = i.add(BigUint::from(1 as u8)); // this is probably slow
        let protocol_name = String::from_utf8(reader.read_var_octet_string()?)?;
        let content_type = ContentType::from(reader.read_u8()?);
        let data = reader.read_var_octet_string()?;
        protocol_data.push(ProtocolData {
            protocol_name,
            content_type,
            data,
        });
    }
    Ok(protocol_data)
}

fn put_protocol_data<T>(buf: &mut T, protocol_data: &[ProtocolData])
where
    T: BufMut,
{
    let length = BigUint::from(protocol_data.len());
    buf.put_var_uint(&length);
    for entry in protocol_data {
        buf.put_var_octet_string(entry.protocol_name.as_bytes());
        buf.put_u8(entry.content_type.clone() as u8);
        buf.put_var_octet_string(&entry.data);
    }
}

#[derive(Debug, PartialEq, Clone)]
pub struct BtpMessage {
    pub request_id: u32,
    pub protocol_data: Vec<ProtocolData>,
}
impl Serializable<BtpMessage> for BtpMessage {
    fn from_bytes(bytes: &[u8]) -> Result<BtpMessage, ParseError> {
        let mut reader = Cursor::new(bytes);
        let packet_type = reader.read_u8()?;
        if PacketType::from(packet_type) != PacketType::Message {
            return Err(ParseError::InvalidPacket(format!(
                "Cannot parse Message from packet of type {}, expected type {}",
                packet_type,
                PacketType::Message as u8
            )));
        }
        let request_id = reader.read_u32::<BigEndian>()?;
        let mut contents = Cursor::new(reader.read_var_octet_string()?);
        let protocol_data = read_protocol_data(&mut contents)?;
        Ok(BtpMessage {
            request_id,
            protocol_data,
        })
    }

    fn to_bytes(&self) -> Vec<u8> {
        let mut buf = Vec::new();
        buf.put_u8(PacketType::Message as u8);
        buf.put_u32_be(self.request_id);
        // TODO make sure this isn't copying the contents
        let mut contents = Vec::new();
        put_protocol_data(&mut contents, &self.protocol_data);
        buf.put_var_octet_string(&contents);
        buf
    }
}

#[derive(Debug, PartialEq, Clone)]
pub struct BtpResponse {
    pub request_id: u32,
    pub protocol_data: Vec<ProtocolData>,
}
impl Serializable<BtpResponse> for BtpResponse {
    fn from_bytes(bytes: &[u8]) -> Result<BtpResponse, ParseError> {
        let mut reader = Cursor::new(bytes);
        let packet_type = reader.read_u8()?;
        if PacketType::from(packet_type) != PacketType::Response {
            return Err(ParseError::InvalidPacket(format!(
                "Cannot parse Response from packet of type {}, expected type {}",
                packet_type,
                PacketType::Response as u8
            )));
        }
        let request_id = reader.read_u32::<BigEndian>()?;
        let mut contents = Cursor::new(reader.read_var_octet_string()?);
        let protocol_data = read_protocol_data(&mut contents)?;
        Ok(BtpResponse {
            request_id,
            protocol_data,
        })
    }

    fn to_bytes(&self) -> Vec<u8> {
        let mut buf = Vec::new();
        buf.put_u8(PacketType::Response as u8);
        buf.put_u32_be(self.request_id);
        let mut contents = Vec::new();
        put_protocol_data(&mut contents, &self.protocol_data);
        buf.put_var_octet_string(&contents);
        buf
    }
}

#[derive(Debug, PartialEq, Clone)]
pub struct BtpError {
    pub request_id: u32,
    pub code: String,
    pub name: String,
    pub triggered_at: DateTime<Utc>,
    pub data: String,
    pub protocol_data: Vec<ProtocolData>,
}
impl Serializable<BtpError> for BtpError {
    fn from_bytes(bytes: &[u8]) -> Result<BtpError, ParseError> {
        let mut reader = Cursor::new(bytes);
        let packet_type = reader.read_u8()?;
        if PacketType::from(packet_type) != PacketType::Error {
            return Err(ParseError::InvalidPacket(format!(
                "Cannot parse Error from packet of type {}, expected type {}",
                packet_type,
                PacketType::Error as u8
            )));
        }
        let request_id = reader.read_u32::<BigEndian>()?;
        let mut contents = Cursor::new(reader.read_var_octet_string()?);
        let mut code: [u8; 3] = [0; 3];
        contents.read_exact(&mut code)?;
        let name = String::from_utf8(contents.read_var_octet_string()?)?;
        let triggered_at_string = String::from_utf8(contents.read_var_octet_string()?)?;
        let triggered_at = Utc.datetime_from_str(&triggered_at_string, GENERALIZED_TIME_FORMAT)?;
        let data = String::from_utf8(contents.read_var_octet_string()?)?;
        let protocol_data = read_protocol_data(&mut contents)?;
        Ok(BtpError {
            request_id,
            code: String::from_utf8(code.to_vec())?,
            name,
            triggered_at,
            data,
            protocol_data,
        })
    }

    fn to_bytes(&self) -> Vec<u8> {
        let mut buf = Vec::new();
        buf.put_u8(PacketType::Error as u8);
        buf.put_u32_be(self.request_id);
        let mut contents = Vec::new();
        // TODO check that the code is only 3 chars
        contents.put(self.code.as_bytes());
        contents.put_var_octet_string(self.name.as_bytes());
        contents.put_var_octet_string(
            self.triggered_at
                .format(GENERALIZED_TIME_FORMAT)
                .to_string()
                .as_bytes(),
        );
        contents.put_var_octet_string(self.data.as_bytes());
        put_protocol_data(&mut contents, &self.protocol_data);
        buf.put_var_octet_string(&contents);
        buf
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hex;

    mod btp_message {
        use super::*;

        static MESSAGE_1: Lazy<BtpMessage> = Lazy::new(|| BtpMessage {
            request_id: 2,
            protocol_data: vec![
                ProtocolData {
                    protocol_name: String::from("test"),
                    content_type: ContentType::ApplicationOctetStream,
                    data: hex::decode("FFFF").unwrap(),
                },
                ProtocolData {
                    protocol_name: String::from("text"),
                    content_type: ContentType::TextPlainUtf8,
                    data: Vec::from("hello"),
                },
            ],
        });
        static MESSAGE_1_SERIALIZED: Lazy<Vec<u8>> = Lazy::new(|| {
            hex::decode("060000000217010204746573740002ffff0474657874010568656c6c6f").unwrap()
        });

        #[test]
        fn from_bytes() {
            assert_eq!(
                BtpMessage::from_bytes(&MESSAGE_1_SERIALIZED).unwrap(),
                *MESSAGE_1
            );
        }

        #[test]
        fn to_bytes() {
            assert_eq!(MESSAGE_1.to_bytes(), *MESSAGE_1_SERIALIZED);
        }
    }

    mod btp_response {
        use super::*;

        static RESPONSE_1: Lazy<BtpResponse> = Lazy::new(|| BtpResponse {
            request_id: 129,
            protocol_data: vec![ProtocolData {
                protocol_name: String::from("some other protocol"),
                content_type: ContentType::ApplicationOctetStream,
                data: hex::decode("AAAAAA").unwrap(),
            }],
        });
        static RESPONSE_1_SERIALIZED: Lazy<Vec<u8>> = Lazy::new(|| {
            hex::decode("01000000811b010113736f6d65206f746865722070726f746f636f6c0003aaaaaa")
                .unwrap()
        });

        #[test]
        fn from_bytes() {
            assert_eq!(
                BtpResponse::from_bytes(&RESPONSE_1_SERIALIZED).unwrap(),
                *RESPONSE_1
            );
        }

        #[test]
        fn to_bytes() {
            assert_eq!(RESPONSE_1.to_bytes(), *RESPONSE_1_SERIALIZED);
        }
    }

    mod btp_error {
        use super::*;

        static ERROR_1: Lazy<BtpError> = Lazy::new(|| BtpError {
            request_id: 501,
            code: String::from("T00"),
            name: String::from("UnreachableError"),
            triggered_at: DateTime::parse_from_rfc3339("2018-08-31T02:53:24.899Z")
                .unwrap()
                .with_timezone(&Utc),
            data: String::from("oops"),
            protocol_data: vec![],
        });

        static ERROR_1_SERIALIZED: Lazy<Vec<u8>> = Lazy::new(|| {
            hex::decode("02000001f52f54303010556e726561636861626c654572726f721332303138303833313032353332342e3839395a046f6f70730100").unwrap()
        });

        #[test]
        fn from_bytes() {
            assert_eq!(BtpError::from_bytes(&ERROR_1_SERIALIZED).unwrap(), *ERROR_1);
        }

        #[test]
        fn to_bytes() {
            assert_eq!(ERROR_1.to_bytes(), *ERROR_1_SERIALIZED);
        }
    }
}
