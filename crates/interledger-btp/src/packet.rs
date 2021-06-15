use super::errors::{BtpPacketError, PacketTypeError};
use bytes::{Buf, BufMut};
use interledger_packet::{
    oer::{self, BufOerExt, MutBufOerExt, VariableLengthTimestamp},
    OerError,
};
#[cfg(test)]
use once_cell::sync::Lazy;
use std::borrow::Cow;
use std::str;

const REQUEST_ID_LEN: usize = 4;

pub trait Serializable<T> {
    fn from_bytes(bytes: &[u8]) -> Result<T, BtpPacketError>;

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

impl PacketType {
    /// Length on the wire.
    const LEN: usize = 1;
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
    fn from_bytes(bytes: &[u8]) -> Result<BtpPacket, BtpPacketError> {
        if bytes.is_empty() {
            return Err(OerError::UnexpectedEof.into());
        }
        match PacketType::from(bytes[0]) {
            PacketType::Message => Ok(BtpPacket::Message(BtpMessage::from_bytes(bytes)?)),
            PacketType::Response => Ok(BtpPacket::Response(BtpResponse::from_bytes(bytes)?)),
            PacketType::Error => Ok(BtpPacket::Error(BtpError::from_bytes(bytes)?)),
            PacketType::Unknown => Err(PacketTypeError::Unknown(bytes[0]).into()),
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

impl ContentType {
    /// Length of the ContentType on the wire
    const LEN: usize = 1;
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

fn read_protocol_data(reader: &mut &[u8]) -> Result<Vec<ProtocolData>, BtpPacketError> {
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

        if reader.remaining() < ContentType::LEN {
            return Err(OerError::UnexpectedEof.into());
        }
        let content_type = ContentType::from(reader.get_u8());
        let data = reader.read_var_octet_string()?.to_vec();
        protocol_data.push(ProtocolData {
            protocol_name,
            content_type,
            data,
        });
    }

    check_no_trailing_bytes(reader)?;

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

fn check_no_trailing_bytes(buf: &[u8]) -> Result<(), BtpPacketError> {
    // according to spec, there should not be room for trailing bytes.
    // this certainly helps with fuzzing.
    if !buf.is_empty() {
        return Err(BtpPacketError::TrailingBytesErr);
    }

    Ok(())
}

#[derive(Debug, PartialEq, Clone)]
pub struct BtpMessage {
    pub request_id: u32,
    pub protocol_data: Vec<ProtocolData>,
}

impl Serializable<BtpMessage> for BtpMessage {
    fn from_bytes(bytes: &[u8]) -> Result<BtpMessage, BtpPacketError> {
        let mut reader = bytes;

        const MIN_LEN: usize = PacketType::LEN + REQUEST_ID_LEN + oer::EMPTY_VARLEN_OCTETS_LEN;

        if reader.remaining() < MIN_LEN {
            return Err(OerError::UnexpectedEof.into());
        }
        let packet_type = reader.get_u8();
        if PacketType::from(packet_type) != PacketType::Message {
            return Err(PacketTypeError::Unexpected(packet_type, PacketType::Message as u8).into());
        }
        let request_id = reader.get_u32();
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
    fn from_bytes(bytes: &[u8]) -> Result<BtpResponse, BtpPacketError> {
        let mut reader = bytes;

        const MIN_LEN: usize = PacketType::LEN + REQUEST_ID_LEN + oer::EMPTY_VARLEN_OCTETS_LEN;

        if reader.remaining() < MIN_LEN {
            return Err(OerError::UnexpectedEof.into());
        }
        let packet_type = reader.get_u8();
        if PacketType::from(packet_type) != PacketType::Response {
            return Err(
                PacketTypeError::Unexpected(packet_type, PacketType::Response as u8).into(),
            );
        }
        let request_id = reader.get_u32();
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
    pub triggered_at: VariableLengthTimestamp,
    pub data: String,
    pub protocol_data: Vec<ProtocolData>,
}
impl Serializable<BtpError> for BtpError {
    fn from_bytes(bytes: &[u8]) -> Result<BtpError, BtpPacketError> {
        let mut reader = bytes;

        const MIN_HEADER_LEN: usize =
            PacketType::LEN + REQUEST_ID_LEN + oer::EMPTY_VARLEN_OCTETS_LEN;

        if reader.remaining() < MIN_HEADER_LEN {
            return Err(OerError::UnexpectedEof.into());
        }
        let packet_type = reader.get_u8();
        if PacketType::from(packet_type) != PacketType::Error {
            return Err(PacketTypeError::Unexpected(packet_type, PacketType::Error as u8).into());
        }
        let request_id = reader.get_u32();
        let mut contents = reader.read_var_octet_string()?;

        check_no_trailing_bytes(reader)?;

        const CODE_LEN: usize = 3;

        const MIN_CONTENTS_LEN: usize = CODE_LEN
            + oer::EMPTY_VARLEN_OCTETS_LEN
            + oer::MIN_VARLEN_TIMESTAMP_LEN
            + oer::EMPTY_VARLEN_OCTETS_LEN;

        if contents.remaining() < MIN_CONTENTS_LEN {
            return Err(OerError::UnexpectedEof.into());
        }

        let mut code = [0u8; CODE_LEN];
        contents.copy_to_slice(&mut code);
        let name = str::from_utf8(contents.read_var_octet_string()?)?.to_owned();
        let triggered_at = contents.read_variable_length_timestamp()?;
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
        contents.put_variable_length_timestamp(&self.triggered_at);
        contents.put_var_octet_string(self.data.as_bytes());
        put_protocol_data(&mut contents, &self.protocol_data);
        buf.put_var_octet_string(&*contents);
        buf
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // separate mod helps to avoid the 30s test case with `cargo test -- fuzzed`
    mod fuzzed {
        use super::super::{put_protocol_data, read_protocol_data};
        use super::BtpPacket;
        use super::Serializable;

        #[test]
        fn fuzz_0_empty_input() {
            fails_to_parse(&[]);
        }

        #[test]
        fn fuzz_1_too_large_uint() {
            fails_to_parse(&[6, 0, 0, 1, 0, 1, 45]);
            //                                 ^^
            //                                  length of uint
        }

        #[test]
        fn fuzz_2_too_short_var_octets() {
            fails_to_parse(&[1, 1, 0, 0, 4, 4, 0]);
            //                           ^ length of var_octet_string
        }

        #[test]
        fn fuzz_3_too_short_var_octets() {
            // 9 is the length of the next section but there are only two bytes, this used to parse
            // just fine because there was no checking for how much was actually read
            // possible duplicate
            fails_to_parse(&[1, 1, 65, 0, 0, 9, 1, 0]);
            //                               ^ length of var_octet_string
        }

        #[test]
        fn fuzz_4_trailing_bytes() {
            // this one has garbage at the end
            fails_to_parse(&[1, 0, 0, 2, 0, 2, 0, 0, 250, 134]);
            //                                       ^^^  ^^^ extra
        }

        #[test]
        fn fuzz_5_trailing_bytes_inside_protocol_data() {
            // this one again has garbage at the end, but inside the protocol data
            fails_to_parse(&[1, 1, 0, 1, 0, 6, 1, 0, 6, 1, 6, 1, 1]);
            //                              /  |  |  ^  ^  ^  ^  ^
            //             protocol data len   |  |    extra
            //                                 /  |
            //                       len of len   /
            //                         num_entries
        }

        #[test]
        fn fuzz_6_too_short_var_octets() {
            fails_to_parse(&[1, 1, 2, 217, 19, 50, 212]);
        }

        #[test]
        fn fuzz_7_too_short_var_octets() {
            fails_to_parse(&[2, 0, 0, 30, 30, 134, 30, 8, 36, 128, 96, 50]);
        }

        #[test]
        fn fuzz_8_large_allocation() {
            // old implementation tries to do malloc(2214616063) here
            fails_to_parse(&[1, 1, 0, 6, 1, 132, 132, 0, 91, 255, 50]);
        }

        #[test]
        fn fuzz_9_short_var_octet_string() {
            // might be duplicate
            #[rustfmt::skip]
            fails_to_parse(&[
                // message
                6,
                // requestid
                0, 0, 1, 1,
                // var octet string len
                6,
                // varuint len
                1,
                // varuint zero
                0,
            ]);
        }

        #[test]
        fn fuzz_10_garbage_protocol_data() {
            // garbage in the protocol data
            // possible duplicate
            fails_to_parse(&[6, 0, 0, 1, 1, 6, 1, 0, 253, 1, 1, 1]);
        }

        #[test]
        fn fuzz_11_failed_roundtrip() {
            // didn't originally record why this failed roundtrip
            roundtrip(&[6, 0, 0, 1, 1, 6, 1, 1, 0, 253, 1, 0]);
        }

        #[test]
        fn fuzz_12_waste_in_length_of_length() {
            // this has a length of length 128 | 1 which means single byte length, which doesn't
            // really make sense per rules
            fails_to_parse(
                &[6, 0, 0, 1, 1, 7, 129, 1, 1, 1, 6, 1, 0],
                //                  ^^^  ^
                //         len of len     string len
            );
        }

        #[test]
        fn fuzz_13_wasteful_var_uint() {
            #[rustfmt::skip]
            fails_on_strict(
                &[
                    // message
                    6,
                    // requestid
                    0, 0, 0, 0,
                    // var octet length
                    6,
                        // length of var uint
                        2,
                        // var uint byte 1/2
                        0,
                        // var uint byte 2/2
                        1,
                        // protocol name, var octet string
                        0,
                        // content type
                        1,
                        // data
                        0],
                &[6, 0, 0, 0, 0, 5, 1, 1, 0, 1, 0],
            );
        }

        #[test]
        fn fuzz_14_invalid_timestamp() {
            // this failed originally by producing a longer output than input in strict.
            //
            // longer output was created because the timestamp was parsed when it contained illegal
            // characters, and the formatted version of the parsed timestamp was longer than in the
            // input.
            #[rustfmt::skip]
            fails_to_parse(&[
                // packettype error
                2,
                // request id
                0, 127, 1, 12, 73,
                // code
                9, 9, 9,
                // name = length prefix + 9x9
                9, 9, 9, 9, 9, 9, 9, 9, 9, 9,
                // timestamp = length prefix (18) + "4\t3\t\u{c}\t\t3\t\u{c}\t5\t3\t60Z"
                //                                  "4.3....3...5.3.60Z"
                // this is output as         (19) + "00040303050360.000Z"
                18, 52, 9, 51, 9, 12, 9, 9, 51, 9, 12, 9, 53, 9, 51, 9, 54, 48, 90,
                // data = length prefix + rest
                0,
                // protocol data
                1, 3, 1, 1, 0, 0, 1, 0, 6, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 79, 9, 9,
                9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 51,
            ]);
        }

        #[test]
        fn fuzz_14_1() {
            // protocol data from the previous test case
            let input: &[u8] = &[
                1, 3, 1, 1, 0, 0, 1, 0, 6, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 79, 9, 9,
                9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 51,
            ];

            let mut cursor = input;

            let pd = read_protocol_data(&mut cursor).unwrap();

            let mut out = bytes::BytesMut::new();

            put_protocol_data(&mut out, &pd);

            assert_eq!(input, out);
        }

        fn fails_on_strict(data: &[u8], lenient_output: &[u8]) {
            let parsed = BtpPacket::from_bytes(data);
            if cfg!(feature = "strict") {
                parsed.unwrap_err();
            } else {
                // without strict, the input is not roundtrippable as it wastes bytes
                let out = parsed.unwrap().to_bytes();
                assert_eq!(out, lenient_output);
            }
        }

        fn fails_to_parse(data: &[u8]) {
            BtpPacket::from_bytes(data).unwrap_err();
        }

        fn roundtrip(data: &[u8]) {
            let parsed = BtpPacket::from_bytes(data).expect("failed to parse test case input");
            let out = parsed.to_bytes();
            assert_eq!(data, out, "{:?}", parsed);
        }
    }

    #[test]
    fn content_type_roundtrips() {
        // this is an important property for any of the datatypes, otherwise fuzzer will find the
        // [^01] examples, which may not have any examples above.
        for x in 0..=255u8 {
            let y: u8 = ContentType::from(x).into();
            assert_eq!(x, y);
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
            triggered_at: VariableLengthTimestamp::parse_from_rfc3339("2018-08-31T02:53:24.899Z")
                .unwrap(),
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
