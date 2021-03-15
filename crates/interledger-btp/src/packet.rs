use super::errors::ParseError;
use byteorder::{BigEndian, ReadBytesExt};
use bytes::BufMut;
use chrono::{DateTime, TimeZone, Utc};
use interledger_packet::oer::{BufOerExt, MutBufOerExt};
#[cfg(test)]
use once_cell::sync::Lazy;
use std::borrow::Cow;
use std::io::prelude::*;
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
        if bytes.is_empty() {
            return Err(ParseError::IoErr(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "too short packet",
            )));
        }
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

#[derive(Debug, PartialEq, Clone, Copy)]
pub enum ContentType {
    ApplicationOctetStream,
    TextPlainUtf8,
    Unknown(u8),
}

impl From<u8> for ContentType {
    fn from(type_int: u8) -> Self {
        match type_int {
            0 => ContentType::ApplicationOctetStream,
            1 => ContentType::TextPlainUtf8,
            x => ContentType::Unknown(x),
        }
    }
}

impl From<ContentType> for u8 {
    fn from(ct: ContentType) -> Self {
        match ct {
            ContentType::ApplicationOctetStream => 0,
            ContentType::TextPlainUtf8 => 1,
            ContentType::Unknown(x) => x,
        }
    }
}

#[derive(Debug, PartialEq, Clone)]
pub struct ProtocolData {
    pub protocol_name: Cow<'static, str>,
    pub content_type: ContentType,
    pub data: Vec<u8>,
}

fn read_protocol_data(reader: &mut &[u8]) -> Result<Vec<ProtocolData>, ParseError> {
    // TODO: using bytes here might make sense
    let mut protocol_data = Vec::new();

    let num_entries = reader.read_var_uint()?;
    for _ in 0..num_entries {
        let protocol_name = str::from_utf8(reader.read_var_octet_string()?)?;

        // avoid allocations for the names contained in the API. if this list needs to be expanded
        // might be better to use phf but this might be still cheaper with 3 equality checks
        let protocol_name = if protocol_name == "ilp" {
            Cow::Borrowed("ilp")
        } else if protocol_name == "auth" {
            Cow::Borrowed("auth")
        } else if protocol_name == "auth_token" {
            Cow::Borrowed("auth_token")
        } else {
            Cow::Owned(protocol_name.to_owned())
        };

        let content_type = ContentType::from(reader.read_u8()?);
        let data = reader.read_var_octet_string()?.to_vec();
        protocol_data.push(ProtocolData {
            protocol_name,
            content_type,
            data,
        });
    }
    Ok(protocol_data)
}

fn put_protocol_data<T: BufMut>(buf: &mut T, protocol_data: &[ProtocolData]) {
    buf.put_var_uint(protocol_data.len() as u64);
    for entry in protocol_data {
        buf.put_var_octet_string(entry.protocol_name.as_bytes());
        buf.put_u8(entry.content_type.into());
        buf.put_var_octet_string(&*entry.data);
    }
}

fn check_no_trailing_bytes(buf: &[u8]) -> Result<(), std::io::Error> {
    // according to spec, there should not be room for trailing bytes.
    // this certainly helps with fuzzing.
    if !buf.is_empty() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "extra trailing bytes",
        ));
    }

    Ok(())
}

#[derive(Debug, PartialEq, Clone)]
pub struct BtpMessage {
    pub request_id: u32,
    pub protocol_data: Vec<ProtocolData>,
}

impl Serializable<BtpMessage> for BtpMessage {
    fn from_bytes(bytes: &[u8]) -> Result<BtpMessage, ParseError> {
        let mut reader = bytes;
        let packet_type = reader.read_u8()?;
        if PacketType::from(packet_type) != PacketType::Message {
            return Err(ParseError::InvalidPacket(format!(
                "Cannot parse Message from packet of type {}, expected type {}",
                packet_type,
                PacketType::Message as u8
            )));
        }
        let request_id = reader.read_u32::<BigEndian>()?;
        let mut contents = reader.read_var_octet_string()?;

        check_no_trailing_bytes(reader)?;

        let protocol_data = read_protocol_data(&mut contents)?;

        Ok(BtpMessage {
            request_id,
            protocol_data,
        })
    }

    fn to_bytes(&self) -> Vec<u8> {
        let mut buf = Vec::new();
        buf.put_u8(PacketType::Message as u8);
        buf.put_u32(self.request_id);
        // TODO make sure this isn't copying the contents
        let mut contents = Vec::new();
        put_protocol_data(&mut contents, &self.protocol_data);
        buf.put_var_octet_string(&*contents);
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
        let mut reader = bytes;
        let packet_type = reader.read_u8()?;
        if PacketType::from(packet_type) != PacketType::Response {
            return Err(ParseError::InvalidPacket(format!(
                "Cannot parse Response from packet of type {}, expected type {}",
                packet_type,
                PacketType::Response as u8
            )));
        }
        let request_id = reader.read_u32::<BigEndian>()?;
        let mut contents = reader.read_var_octet_string()?;

        check_no_trailing_bytes(reader)?;

        let protocol_data = read_protocol_data(&mut contents)?;
        Ok(BtpResponse {
            request_id,
            protocol_data,
        })
    }

    fn to_bytes(&self) -> Vec<u8> {
        let mut buf = Vec::new();
        buf.put_u8(PacketType::Response as u8);
        buf.put_u32(self.request_id);
        let mut contents = Vec::new();
        put_protocol_data(&mut contents, &self.protocol_data);
        buf.put_var_octet_string(&*contents);
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
        let mut reader = bytes;
        let packet_type = reader.read_u8()?;
        if PacketType::from(packet_type) != PacketType::Error {
            return Err(ParseError::InvalidPacket(format!(
                "Cannot parse Error from packet of type {}, expected type {}",
                packet_type,
                PacketType::Error as u8
            )));
        }
        let request_id = reader.read_u32::<BigEndian>()?;
        let mut contents = reader.read_var_octet_string()?;

        check_no_trailing_bytes(reader)?;

        let mut code: [u8; 3] = [0; 3];
        contents.read_exact(&mut code)?;
        let name = str::from_utf8(contents.read_var_octet_string()?)?.to_owned();
        let triggered_at_string = str::from_utf8(contents.read_var_octet_string()?)?.to_owned();
        let triggered_at = Utc.datetime_from_str(&triggered_at_string, GENERALIZED_TIME_FORMAT)?;
        let data = str::from_utf8(contents.read_var_octet_string()?)?.to_owned();
        let protocol_data = read_protocol_data(&mut contents)?;
        Ok(BtpError {
            request_id,
            code: str::from_utf8(&code[..])?.to_owned(),
            name,
            triggered_at,
            data,
            protocol_data,
        })
    }

    fn to_bytes(&self) -> Vec<u8> {
        let mut buf = Vec::new();
        buf.put_u8(PacketType::Error as u8);
        buf.put_u32(self.request_id);
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
        buf.put_var_octet_string(&*contents);
        buf
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    mod fuzzed {
        use super::BtpPacket;
        use super::Serializable;

        #[test]
        fn fuzz_0() {
            fails_to_parse(&[]);
        }

        #[test]
        fn fuzz_1() {
            fails_to_parse(&[6, 0, 0, 1, 0, 1, 45]);
        }

        #[test]
        fn fuzz_2() {
            fails_to_parse(&[1, 1, 0, 0, 4, 4, 0]);
        }

        #[test]
        fn fuzz_3() {
            // 9 is the length of the next section but there are only two bytes, this used to parse
            // just fine because there was no checking for how much was actually read
            fails_to_parse(&[1, 1, 65, 0, 0, 9, 1, 0]);
        }

        #[test]
        fn fuzz_4() {
            // this one has garbage at the end
            fails_to_parse(&[1, 0, 0, 2, 0, 2, 0, 0, 250, 134]);
        }

        #[test]
        fn fuzz_5() {
            // this one again has garbage at the end, but inside the protocol data
            fails_to_parse(&[1, 1, 0, 1, 0, 6, 1, 0, 6, 1, 6, 1, 1]);
            //                                 /  |
            //                       len of len   /
            //                         num_entries
        }

        #[test]
        fn fuzz_6() {
            fails_to_parse(&[1, 1, 2, 217, 19, 50, 212]);
        }

        #[test]
        fn fuzz_7() {
            fails_to_parse(&[2, 0, 0, 30, 30, 134, 30, 8, 36, 128, 96, 50]);
        }

        #[test]
        fn fuzz_8() {
            // old implementation tries to do malloc(2214616063) here
            fails_to_parse(&[1, 1, 0, 6, 1, 132, 132, 0, 91, 255, 50]);
        }

        fn fails_to_parse(data: &[u8]) {
            BtpPacket::from_bytes(data).unwrap_err();
        }

        #[allow(unused)]
        fn roundtrip(data: &[u8]) {
            let parsed = BtpPacket::from_bytes(data).expect("failed to parse test case input");
            let out = parsed.to_bytes();
            assert_eq!(data, out, "{:?}", parsed);
        }
    }

    mod btp_message {
        use super::*;

        static MESSAGE_1: Lazy<BtpMessage> = Lazy::new(|| BtpMessage {
            request_id: 2,
            protocol_data: vec![
                ProtocolData {
                    protocol_name: "test".into(),
                    content_type: ContentType::ApplicationOctetStream,
                    data: hex_literal::hex!("FFFF")[..].to_vec(),
                },
                ProtocolData {
                    protocol_name: "text".into(),
                    content_type: ContentType::TextPlainUtf8,
                    data: b"hello".to_vec(),
                },
            ],
        });
        static MESSAGE_1_SERIALIZED: &[u8] =
            &hex_literal::hex!("060000000217010204746573740002ffff0474657874010568656c6c6f");

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
                protocol_name: "some other protocol".into(),
                content_type: ContentType::ApplicationOctetStream,
                data: hex_literal::hex!("AAAAAA").to_vec(),
            }],
        });
        static RESPONSE_1_SERIALIZED: &[u8] = &hex_literal::hex!(
            "01000000811b010113736f6d65206f746865722070726f746f636f6c0003aaaaaa"
        );

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

        static ERROR_1_SERIALIZED: &[u8] = &hex_literal::hex!("02000001f52f54303010556e726561636861626c654572726f721332303138303833313032353332342e3839395a046f6f70730100");

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
