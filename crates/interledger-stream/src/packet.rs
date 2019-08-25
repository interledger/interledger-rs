use super::crypto::{decrypt, encrypt};
use byteorder::ReadBytesExt;
use bytes::{BufMut, BytesMut};
use interledger_packet::{
    oer::{BufOerExt, MutBufOerExt},
    Address, PacketType as IlpPacketType, ParseError,
};
#[cfg(test)]
use lazy_static::lazy_static;
use log::warn;
use std::{convert::TryFrom, fmt, str, u64};

const STREAM_VERSION: u8 = 1;

pub struct StreamPacketBuilder<'a> {
    pub sequence: u64,
    pub ilp_packet_type: IlpPacketType,
    pub prepare_amount: u64,
    pub frames: &'a [Frame<'a>],
}

impl<'a> StreamPacketBuilder<'a> {
    pub fn build(&self) -> StreamPacket {
        // TODO predict length first
        let mut buffer_unencrypted = Vec::new();

        buffer_unencrypted.put_u8(STREAM_VERSION);
        buffer_unencrypted.put_u8(self.ilp_packet_type as u8);
        buffer_unencrypted.put_var_uint(self.sequence);
        buffer_unencrypted.put_var_uint(self.prepare_amount);
        buffer_unencrypted.put_var_uint(self.frames.len() as u64);
        let frames_offset = buffer_unencrypted.len();

        for frame in self.frames {
            let mut contents = Vec::new();
            match frame {
                Frame::ConnectionClose(ref frame) => {
                    buffer_unencrypted.put_u8(FrameType::ConnectionClose as u8);
                    frame.put_contents(&mut contents);
                }
                Frame::ConnectionNewAddress(ref frame) => {
                    buffer_unencrypted.put_u8(FrameType::ConnectionNewAddress as u8);
                    frame.put_contents(&mut contents);
                }
                Frame::ConnectionAssetDetails(ref frame) => {
                    buffer_unencrypted.put_u8(FrameType::ConnectionAssetDetails as u8);
                    frame.put_contents(&mut contents);
                }
                Frame::ConnectionMaxData(ref frame) => {
                    buffer_unencrypted.put_u8(FrameType::ConnectionMaxData as u8);
                    frame.put_contents(&mut contents);
                }
                Frame::ConnectionDataBlocked(ref frame) => {
                    buffer_unencrypted.put_u8(FrameType::ConnectionDataBlocked as u8);
                    frame.put_contents(&mut contents);
                }
                Frame::ConnectionMaxStreamId(ref frame) => {
                    buffer_unencrypted.put_u8(FrameType::ConnectionMaxStreamId as u8);
                    frame.put_contents(&mut contents);
                }
                Frame::ConnectionStreamIdBlocked(ref frame) => {
                    buffer_unencrypted.put_u8(FrameType::ConnectionStreamIdBlocked as u8);
                    frame.put_contents(&mut contents);
                }
                Frame::StreamClose(ref frame) => {
                    buffer_unencrypted.put_u8(FrameType::StreamClose as u8);
                    frame.put_contents(&mut contents);
                }
                Frame::StreamMoney(ref frame) => {
                    buffer_unencrypted.put_u8(FrameType::StreamMoney as u8);
                    frame.put_contents(&mut contents);
                }
                Frame::StreamMaxMoney(ref frame) => {
                    buffer_unencrypted.put_u8(FrameType::StreamMaxMoney as u8);
                    frame.put_contents(&mut contents);
                }
                Frame::StreamMoneyBlocked(ref frame) => {
                    buffer_unencrypted.put_u8(FrameType::StreamMoneyBlocked as u8);
                    frame.put_contents(&mut contents);
                }
                Frame::StreamData(ref frame) => {
                    buffer_unencrypted.put_u8(FrameType::StreamData as u8);
                    frame.put_contents(&mut contents);
                }
                Frame::StreamMaxData(ref frame) => {
                    buffer_unencrypted.put_u8(FrameType::StreamMaxData as u8);
                    frame.put_contents(&mut contents);
                }
                Frame::StreamDataBlocked(ref frame) => {
                    buffer_unencrypted.put_u8(FrameType::StreamDataBlocked as u8);
                    frame.put_contents(&mut contents);
                }
                Frame::Unknown => continue,
            }
            buffer_unencrypted.put_var_octet_string(contents);
        }

        StreamPacket {
            buffer_unencrypted: BytesMut::from(buffer_unencrypted),
            sequence: self.sequence,
            ilp_packet_type: self.ilp_packet_type,
            prepare_amount: self.prepare_amount,
            frames_offset,
        }
    }
}

#[derive(PartialEq, Clone)]
pub struct StreamPacket {
    pub(crate) buffer_unencrypted: BytesMut,
    sequence: u64,
    ilp_packet_type: IlpPacketType,
    prepare_amount: u64,
    frames_offset: usize,
}

impl StreamPacket {
    pub fn from_encrypted(shared_secret: &[u8], ciphertext: BytesMut) -> Result<Self, ParseError> {
        // TODO handle decryption failure
        let decrypted = decrypt(shared_secret, ciphertext)
            .map_err(|_err| ParseError::InvalidPacket(String::from("Unable to decrypt packet")))?;
        StreamPacket::from_bytes_unencrypted(decrypted)
    }

    fn from_bytes_unencrypted(buffer_unencrypted: BytesMut) -> Result<Self, ParseError> {
        // TODO don't copy the whole packet again
        let mut reader = &buffer_unencrypted[..];
        let version = reader.read_u8()?;
        if version != STREAM_VERSION {
            return Err(ParseError::InvalidPacket(format!(
                "Unsupported STREAM version: {}",
                version
            )));
        }
        let ilp_packet_type = IlpPacketType::try_from(reader.read_u8()?)?;
        let sequence = reader.read_var_uint()?;
        let prepare_amount = reader.read_var_uint()?;

        // TODO save num_frames?
        let num_frames = reader.read_var_uint()?;
        let frames_offset = buffer_unencrypted.len() - reader.len();

        // Try reading through all the frames to make sure they can be parsed correctly
        if num_frames
            == (FrameIterator {
                buffer: &buffer_unencrypted[frames_offset..],
            })
            .count() as u64
        {
            Ok(StreamPacket {
                buffer_unencrypted,
                sequence,
                ilp_packet_type,
                prepare_amount,
                frames_offset,
            })
        } else {
            Err(ParseError::InvalidPacket(
                "Incorrect number of frames or unable to parse all frames".to_string(),
            ))
        }
    }

    pub fn into_encrypted(self, shared_secret: &[u8]) -> BytesMut {
        encrypt(shared_secret, self.buffer_unencrypted)
    }

    pub fn sequence(&self) -> u64 {
        self.sequence
    }

    pub fn ilp_packet_type(&self) -> IlpPacketType {
        self.ilp_packet_type
    }

    pub fn prepare_amount(&self) -> u64 {
        self.prepare_amount
    }

    pub fn frames(&self) -> FrameIterator {
        FrameIterator {
            buffer: &self.buffer_unencrypted[self.frames_offset..],
        }
    }
}

impl fmt::Debug for StreamPacket {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(
            f,
            "StreamPacket {{ sequence: {}, ilp_packet_type: {:?}, prepare_amount: {}, frames: {:?} }}",
            self.sequence,
            self.ilp_packet_type,
            self.prepare_amount,
            self.frames()
        )
    }
}

pub struct FrameIterator<'a> {
    buffer: &'a [u8],
}

impl<'a> FrameIterator<'a> {
    fn try_read_next_frame(&mut self) -> Result<Frame<'a>, ParseError> {
        let frame_type = self.buffer.read_u8()?;
        let contents: &'a [u8] = self.buffer.read_var_octet_string()?;
        let frame: Frame<'a> = match FrameType::from(frame_type) {
            FrameType::ConnectionClose => {
                Frame::ConnectionClose(ConnectionCloseFrame::read_contents(&contents)?)
            }
            FrameType::ConnectionNewAddress => {
                Frame::ConnectionNewAddress(ConnectionNewAddressFrame::read_contents(&contents)?)
            }
            FrameType::ConnectionAssetDetails => Frame::ConnectionAssetDetails(
                ConnectionAssetDetailsFrame::read_contents(&contents)?,
            ),
            FrameType::ConnectionMaxData => {
                Frame::ConnectionMaxData(ConnectionMaxDataFrame::read_contents(&contents)?)
            }
            FrameType::ConnectionDataBlocked => {
                Frame::ConnectionDataBlocked(ConnectionDataBlockedFrame::read_contents(&contents)?)
            }
            FrameType::ConnectionMaxStreamId => {
                Frame::ConnectionMaxStreamId(ConnectionMaxStreamIdFrame::read_contents(&contents)?)
            }
            FrameType::ConnectionStreamIdBlocked => Frame::ConnectionStreamIdBlocked(
                ConnectionStreamIdBlockedFrame::read_contents(&contents)?,
            ),
            FrameType::StreamClose => {
                Frame::StreamClose(StreamCloseFrame::read_contents(&contents)?)
            }
            FrameType::StreamMoney => {
                Frame::StreamMoney(StreamMoneyFrame::read_contents(&contents)?)
            }
            FrameType::StreamMaxMoney => {
                Frame::StreamMaxMoney(StreamMaxMoneyFrame::read_contents(&contents)?)
            }
            FrameType::StreamMoneyBlocked => {
                Frame::StreamMoneyBlocked(StreamMoneyBlockedFrame::read_contents(&contents)?)
            }
            FrameType::StreamData => Frame::StreamData(StreamDataFrame::read_contents(&contents)?),
            FrameType::StreamMaxData => {
                Frame::StreamMaxData(StreamMaxDataFrame::read_contents(&contents)?)
            }
            FrameType::StreamDataBlocked => {
                Frame::StreamDataBlocked(StreamDataBlockedFrame::read_contents(&contents)?)
            }
            FrameType::Unknown => {
                warn!(
                    "Ignoring unknown frame of type {}: {:x?}",
                    frame_type, contents,
                );
                Frame::Unknown
            }
        };

        Ok(frame)
    }
}

impl<'a> Iterator for FrameIterator<'a> {
    type Item = Frame<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        while !self.buffer.is_empty() {
            // TODO don't ignore errors if the packet is just invalid
            match self.try_read_next_frame() {
                Ok(frame) => return Some(frame),
                Err(err) => warn!("Error reading STREAM frame: {:?}", err),
            }
        }

        None
    }
}

impl<'a> fmt::Debug for FrameIterator<'a> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "[ ")?;
        let mut iter = FrameIterator {
            buffer: &self.buffer[..],
        };
        if let Some(next) = iter.next() {
            write!(f, "{:?}", next)?;
        }
        for frame in iter {
            write!(f, ", {:?}", frame)?;
        }
        write!(f, " ]")
    }
}

#[derive(PartialEq, Clone)]
pub enum Frame<'a> {
    ConnectionClose(ConnectionCloseFrame<'a>),
    ConnectionNewAddress(ConnectionNewAddressFrame),
    ConnectionAssetDetails(ConnectionAssetDetailsFrame<'a>),
    ConnectionMaxData(ConnectionMaxDataFrame),
    ConnectionDataBlocked(ConnectionDataBlockedFrame),
    ConnectionMaxStreamId(ConnectionMaxStreamIdFrame),
    ConnectionStreamIdBlocked(ConnectionStreamIdBlockedFrame),
    StreamClose(StreamCloseFrame<'a>),
    StreamMoney(StreamMoneyFrame),
    StreamMaxMoney(StreamMaxMoneyFrame),
    StreamMoneyBlocked(StreamMoneyBlockedFrame),
    StreamData(StreamDataFrame<'a>),
    StreamMaxData(StreamMaxDataFrame),
    StreamDataBlocked(StreamDataBlockedFrame),
    Unknown,
}

impl<'a> fmt::Debug for Frame<'a> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Frame::ConnectionClose(frame) => write!(f, "{:?}", frame),
            Frame::ConnectionNewAddress(frame) => write!(f, "{:?}", frame),
            Frame::ConnectionAssetDetails(frame) => write!(f, "{:?}", frame),
            Frame::ConnectionMaxData(frame) => write!(f, "{:?}", frame),
            Frame::ConnectionDataBlocked(frame) => write!(f, "{:?}", frame),
            Frame::ConnectionMaxStreamId(frame) => write!(f, "{:?}", frame),
            Frame::ConnectionStreamIdBlocked(frame) => write!(f, "{:?}", frame),
            Frame::StreamClose(frame) => write!(f, "{:?}", frame),
            Frame::StreamMoney(frame) => write!(f, "{:?}", frame),
            Frame::StreamMaxMoney(frame) => write!(f, "{:?}", frame),
            Frame::StreamMoneyBlocked(frame) => write!(f, "{:?}", frame),
            Frame::StreamData(frame) => write!(f, "{:?}", frame),
            Frame::StreamMaxData(frame) => write!(f, "{:?}", frame),
            Frame::StreamDataBlocked(frame) => write!(f, "{:?}", frame),
            Frame::Unknown => write!(f, "UnknownFrame"),
        }
    }
}

#[derive(Debug, PartialEq, Clone)]
#[repr(u8)]
pub enum FrameType {
    ConnectionClose = 0x01,
    ConnectionNewAddress = 0x02,
    ConnectionMaxData = 0x03,
    ConnectionDataBlocked = 0x04,
    ConnectionMaxStreamId = 0x05,
    ConnectionStreamIdBlocked = 0x06,
    ConnectionAssetDetails = 0x07,
    StreamClose = 0x10,
    StreamMoney = 0x11,
    StreamMaxMoney = 0x12,
    StreamMoneyBlocked = 0x13,
    StreamData = 0x14,
    StreamMaxData = 0x15,
    StreamDataBlocked = 0x16,
    Unknown,
}
impl From<u8> for FrameType {
    fn from(num: u8) -> Self {
        match num {
            0x01 => FrameType::ConnectionClose,
            0x02 => FrameType::ConnectionNewAddress,
            0x03 => FrameType::ConnectionMaxData,
            0x04 => FrameType::ConnectionDataBlocked,
            0x05 => FrameType::ConnectionMaxStreamId,
            0x06 => FrameType::ConnectionStreamIdBlocked,
            0x07 => FrameType::ConnectionAssetDetails,
            0x10 => FrameType::StreamClose,
            0x11 => FrameType::StreamMoney,
            0x12 => FrameType::StreamMaxMoney,
            0x13 => FrameType::StreamMoneyBlocked,
            0x14 => FrameType::StreamData,
            0x15 => FrameType::StreamMaxData,
            0x16 => FrameType::StreamDataBlocked,
            _ => FrameType::Unknown,
        }
    }
}

#[derive(Debug, PartialEq, Clone)]
#[repr(u8)]
pub enum ErrorCode {
    NoError = 0x01,
    InternalError = 0x02,
    EndpointBusy = 0x03,
    FlowControlError = 0x04,
    StreamIdError = 0x05,
    StreamStateError = 0x06,
    FrameFormatError = 0x07,
    ProtocolViolation = 0x08,
    ApplicationError = 0x09,
    Unknown,
}
impl From<u8> for ErrorCode {
    fn from(num: u8) -> Self {
        match num {
            0x01 => ErrorCode::NoError,
            0x02 => ErrorCode::InternalError,
            0x03 => ErrorCode::EndpointBusy,
            0x04 => ErrorCode::FlowControlError,
            0x05 => ErrorCode::StreamIdError,
            0x06 => ErrorCode::StreamStateError,
            0x07 => ErrorCode::FrameFormatError,
            0x08 => ErrorCode::ProtocolViolation,
            0x09 => ErrorCode::ApplicationError,
            _ => ErrorCode::Unknown,
        }
    }
}

pub trait SerializableFrame<'a>: Sized {
    fn put_contents(&self, buf: &mut impl MutBufOerExt) -> ();

    fn read_contents(reader: &'a [u8]) -> Result<Self, ParseError>;
}

#[derive(Debug, PartialEq, Clone)]
pub struct ConnectionCloseFrame<'a> {
    pub code: ErrorCode,
    pub message: &'a str,
}

impl<'a> SerializableFrame<'a> for ConnectionCloseFrame<'a> {
    fn read_contents(mut reader: &'a [u8]) -> Result<Self, ParseError> {
        let code = ErrorCode::from(reader.read_u8()?);
        let message_bytes = reader.read_var_octet_string()?;
        let message = str::from_utf8(message_bytes)?;

        Ok(ConnectionCloseFrame { code, message })
    }

    fn put_contents(&self, buf: &mut impl MutBufOerExt) {
        buf.put_u8(self.code.clone() as u8);
        buf.put_var_octet_string(self.message.as_bytes());
    }
}

#[derive(PartialEq, Clone)]
pub struct ConnectionNewAddressFrame {
    pub source_account: Address,
}

impl<'a> SerializableFrame<'a> for ConnectionNewAddressFrame {
    fn read_contents(mut reader: &'a [u8]) -> Result<Self, ParseError> {
        let source_account = reader.read_var_octet_string()?;
        let source_account = Address::try_from(source_account)?;

        Ok(ConnectionNewAddressFrame { source_account })
    }

    fn put_contents(&self, buf: &mut impl MutBufOerExt) {
        let data: &[u8] = self.source_account.as_ref();
        buf.put_var_octet_string(data);
    }
}

impl<'a> fmt::Debug for ConnectionNewAddressFrame {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(
            f,
            "ConnectionNewAddressFrame {{ source_account: {} }}",
            self.source_account,
        )
    }
}

#[derive(Debug, PartialEq, Clone)]
pub struct ConnectionAssetDetailsFrame<'a> {
    pub source_asset_code: &'a str,
    pub source_asset_scale: u8,
}

impl<'a> SerializableFrame<'a> for ConnectionAssetDetailsFrame<'a> {
    fn read_contents(mut reader: &'a [u8]) -> Result<Self, ParseError> {
        let source_asset_code = str::from_utf8(reader.read_var_octet_string()?)?;
        let source_asset_scale = reader.read_u8()?;

        Ok(ConnectionAssetDetailsFrame {
            source_asset_scale,
            source_asset_code,
        })
    }

    fn put_contents(&self, buf: &mut impl MutBufOerExt) {
        buf.put_var_octet_string(self.source_asset_code.as_bytes());
        buf.put_u8(self.source_asset_scale);
    }
}

#[derive(Debug, PartialEq, Clone)]
pub struct ConnectionMaxDataFrame {
    pub max_offset: u64,
}

impl<'a> SerializableFrame<'a> for ConnectionMaxDataFrame {
    fn read_contents(mut reader: &[u8]) -> Result<Self, ParseError> {
        let max_offset = reader.read_var_uint()?;

        Ok(ConnectionMaxDataFrame { max_offset })
    }

    fn put_contents(&self, buf: &mut impl MutBufOerExt) {
        buf.put_var_uint(self.max_offset);
    }
}

#[derive(Debug, PartialEq, Clone)]
pub struct ConnectionDataBlockedFrame {
    pub max_offset: u64,
}

impl<'a> SerializableFrame<'a> for ConnectionDataBlockedFrame {
    fn read_contents(mut reader: &[u8]) -> Result<Self, ParseError> {
        let max_offset = reader.read_var_uint()?;

        Ok(ConnectionDataBlockedFrame { max_offset })
    }

    fn put_contents(&self, buf: &mut impl MutBufOerExt) {
        buf.put_var_uint(self.max_offset);
    }
}

#[derive(Debug, PartialEq, Clone)]
pub struct ConnectionMaxStreamIdFrame {
    pub max_stream_id: u64,
}

impl<'a> SerializableFrame<'a> for ConnectionMaxStreamIdFrame {
    fn read_contents(mut reader: &[u8]) -> Result<Self, ParseError> {
        let max_stream_id = reader.read_var_uint()?;

        Ok(ConnectionMaxStreamIdFrame { max_stream_id })
    }

    fn put_contents(&self, buf: &mut impl MutBufOerExt) {
        buf.put_var_uint(self.max_stream_id);
    }
}

#[derive(Debug, PartialEq, Clone)]
pub struct ConnectionStreamIdBlockedFrame {
    pub max_stream_id: u64,
}

impl<'a> SerializableFrame<'a> for ConnectionStreamIdBlockedFrame {
    fn read_contents(mut reader: &[u8]) -> Result<Self, ParseError> {
        let max_stream_id = reader.read_var_uint()?;

        Ok(ConnectionStreamIdBlockedFrame { max_stream_id })
    }

    fn put_contents(&self, buf: &mut impl MutBufOerExt) {
        buf.put_var_uint(self.max_stream_id);
    }
}

#[derive(Debug, PartialEq, Clone)]
pub struct StreamCloseFrame<'a> {
    pub stream_id: u64,
    pub code: ErrorCode,
    pub message: &'a str,
}

impl<'a> SerializableFrame<'a> for StreamCloseFrame<'a> {
    fn read_contents(mut reader: &'a [u8]) -> Result<Self, ParseError> {
        let stream_id = reader.read_var_uint()?;
        let code = ErrorCode::from(reader.read_u8()?);
        let message_bytes = reader.read_var_octet_string()?;
        let message = str::from_utf8(message_bytes)?;

        Ok(StreamCloseFrame {
            stream_id,
            code,
            message,
        })
    }

    fn put_contents(&self, buf: &mut impl MutBufOerExt) {
        buf.put_var_uint(self.stream_id);
        buf.put_u8(self.code.clone() as u8);
        buf.put_var_octet_string(self.message.as_bytes());
    }
}

#[derive(Debug, PartialEq, Clone)]
pub struct StreamMoneyFrame {
    pub stream_id: u64,
    pub shares: u64,
}

impl<'a> SerializableFrame<'a> for StreamMoneyFrame {
    fn read_contents(mut reader: &[u8]) -> Result<Self, ParseError> {
        let stream_id = reader.read_var_uint()?;
        let shares = reader.read_var_uint()?;

        Ok(StreamMoneyFrame { stream_id, shares })
    }

    fn put_contents(&self, buf: &mut impl MutBufOerExt) {
        buf.put_var_uint(self.stream_id);
        buf.put_var_uint(self.shares);
    }
}

#[derive(Debug, PartialEq, Clone)]
pub struct StreamMaxMoneyFrame {
    pub stream_id: u64,
    pub receive_max: u64,
    pub total_received: u64,
}

impl<'a> SerializableFrame<'a> for StreamMaxMoneyFrame {
    fn read_contents(mut reader: &[u8]) -> Result<Self, ParseError> {
        let stream_id = reader.read_var_uint()?;
        let receive_max = saturating_read_var_uint(&mut reader)?;
        let total_received = reader.read_var_uint()?;

        Ok(StreamMaxMoneyFrame {
            stream_id,
            receive_max,
            total_received,
        })
    }

    fn put_contents(&self, buf: &mut impl MutBufOerExt) {
        buf.put_var_uint(self.stream_id);
        buf.put_var_uint(self.receive_max);
        buf.put_var_uint(self.total_received);
    }
}

#[derive(Debug, PartialEq, Clone)]
pub struct StreamMoneyBlockedFrame {
    pub stream_id: u64,
    pub send_max: u64,
    pub total_sent: u64,
}

impl<'a> SerializableFrame<'a> for StreamMoneyBlockedFrame {
    fn read_contents(mut reader: &[u8]) -> Result<Self, ParseError> {
        let stream_id = reader.read_var_uint()?;
        let send_max = saturating_read_var_uint(&mut reader)?;
        let total_sent = reader.read_var_uint()?;

        Ok(StreamMoneyBlockedFrame {
            stream_id,
            send_max,
            total_sent,
        })
    }

    fn put_contents(&self, buf: &mut impl MutBufOerExt) {
        buf.put_var_uint(self.stream_id);
        buf.put_var_uint(self.send_max);
        buf.put_var_uint(self.total_sent);
    }
}

#[derive(Debug, PartialEq, Clone)]
pub struct StreamDataFrame<'a> {
    pub stream_id: u64,
    pub offset: u64,
    pub data: &'a [u8],
}

impl<'a> SerializableFrame<'a> for StreamDataFrame<'a> {
    fn read_contents(mut reader: &'a [u8]) -> Result<Self, ParseError> {
        let stream_id = reader.read_var_uint()?;
        let offset = reader.read_var_uint()?;
        let data = reader.read_var_octet_string()?;

        Ok(StreamDataFrame {
            stream_id,
            offset,
            data,
        })
    }

    fn put_contents(&self, buf: &mut impl MutBufOerExt) {
        buf.put_var_uint(self.stream_id);
        buf.put_var_uint(self.offset);
        buf.put_var_octet_string(self.data);
    }
}

#[derive(Debug, PartialEq, Clone)]
pub struct StreamMaxDataFrame {
    pub stream_id: u64,
    pub max_offset: u64,
}

impl<'a> SerializableFrame<'a> for StreamMaxDataFrame {
    fn read_contents(mut reader: &[u8]) -> Result<Self, ParseError> {
        let stream_id = reader.read_var_uint()?;
        let max_offset = reader.read_var_uint()?;

        Ok(StreamMaxDataFrame {
            stream_id,
            max_offset,
        })
    }

    fn put_contents(&self, buf: &mut impl MutBufOerExt) {
        buf.put_var_uint(self.stream_id);
        buf.put_var_uint(self.max_offset);
    }
}

#[derive(Debug, PartialEq, Clone)]
pub struct StreamDataBlockedFrame {
    pub stream_id: u64,
    pub max_offset: u64,
}

impl<'a> SerializableFrame<'a> for StreamDataBlockedFrame {
    fn read_contents(mut reader: &[u8]) -> Result<Self, ParseError> {
        let stream_id = reader.read_var_uint()?;
        let max_offset = reader.read_var_uint()?;

        Ok(StreamDataBlockedFrame {
            stream_id,
            max_offset,
        })
    }

    fn put_contents(&self, buf: &mut impl MutBufOerExt) {
        buf.put_var_uint(self.stream_id);
        buf.put_var_uint(self.max_offset);
    }
}

/// See: https://github.com/interledger/rfcs/blob/master/0029-stream/0029-stream.md#514-maximum-varuint-size
fn saturating_read_var_uint<'a>(reader: &mut impl BufOerExt<'a>) -> Result<u64, ParseError> {
    if reader.peek_var_octet_string()?.len() > 8 {
        reader.skip_var_octet_string()?;
        Ok(u64::MAX)
    } else {
        Ok(reader.read_var_uint()?)
    }
}

#[cfg(test)]
mod serialization {
    use super::*;
    use std::str::FromStr;

    lazy_static! {
        static ref PACKET: StreamPacket = StreamPacketBuilder {
            sequence: 1,
            ilp_packet_type: IlpPacketType::try_from(12).unwrap(),
            prepare_amount: 99,
            frames: &[
                Frame::ConnectionClose(ConnectionCloseFrame {
                    code: ErrorCode::NoError,
                    message: "oop"
                }),
                Frame::ConnectionNewAddress(ConnectionNewAddressFrame {
                    source_account: Address::from_str("example.blah").unwrap()
                }),
                Frame::ConnectionMaxData(ConnectionMaxDataFrame { max_offset: 1000 }),
                Frame::ConnectionDataBlocked(ConnectionDataBlockedFrame { max_offset: 2000 }),
                Frame::ConnectionMaxStreamId(ConnectionMaxStreamIdFrame {
                    max_stream_id: 3000,
                }),
                Frame::ConnectionStreamIdBlocked(ConnectionStreamIdBlockedFrame {
                    max_stream_id: 4000,
                }),
                Frame::ConnectionAssetDetails(ConnectionAssetDetailsFrame {
                    source_asset_code: "XYZ",
                    source_asset_scale: 9
                }),
                Frame::StreamClose(StreamCloseFrame {
                    stream_id: 76,
                    code: ErrorCode::InternalError,
                    message: "blah",
                }),
                Frame::StreamMoney(StreamMoneyFrame {
                    stream_id: 88,
                    shares: 99,
                }),
                Frame::StreamMaxMoney(StreamMaxMoneyFrame {
                    stream_id: 11,
                    receive_max: 987,
                    total_received: 500,
                }),
                Frame::StreamMoneyBlocked(StreamMoneyBlockedFrame {
                    stream_id: 66,
                    send_max: 20000,
                    total_sent: 6000,
                }),
                Frame::StreamData(StreamDataFrame {
                    stream_id: 34,
                    offset: 9000,
                    data: b"hello",
                }),
                Frame::StreamMaxData(StreamMaxDataFrame {
                    stream_id: 35,
                    max_offset: 8766
                }),
                Frame::StreamDataBlocked(StreamDataBlockedFrame {
                    stream_id: 888,
                    max_offset: 44444
                }),
            ]
        }
        .build();
        static ref SERIALIZED: BytesMut = BytesMut::from(vec![
            1, 12, 1, 1, 1, 99, 1, 14, 1, 5, 1, 3, 111, 111, 112, 2, 13, 12, 101, 120, 97, 109,
            112, 108, 101, 46, 98, 108, 97, 104, 3, 3, 2, 3, 232, 4, 3, 2, 7, 208, 5, 3, 2, 11,
            184, 6, 3, 2, 15, 160, 7, 5, 3, 88, 89, 90, 9, 16, 8, 1, 76, 2, 4, 98, 108, 97, 104,
            17, 4, 1, 88, 1, 99, 18, 8, 1, 11, 2, 3, 219, 2, 1, 244, 19, 8, 1, 66, 2, 78, 32, 2,
            23, 112, 20, 11, 1, 34, 2, 35, 40, 5, 104, 101, 108, 108, 111, 21, 5, 1, 35, 2, 34, 62,
            22, 6, 2, 3, 120, 2, 173, 156
        ]);
    }

    #[test]
    fn it_serializes_to_same_as_javascript() {
        assert_eq!(PACKET.buffer_unencrypted, *SERIALIZED);
    }

    #[test]
    fn it_deserializes_from_javascript() {
        assert_eq!(
            StreamPacket::from_bytes_unencrypted(SERIALIZED.clone()).unwrap(),
            *PACKET
        );
    }

    #[test]
    fn it_iterates_through_the_frames() {
        let mut iter = PACKET.frames();
        assert_eq!(
            iter.next().unwrap(),
            Frame::ConnectionClose(ConnectionCloseFrame {
                code: ErrorCode::NoError,
                message: "oop"
            })
        );

        assert_eq!(
            iter.next().unwrap(),
            Frame::ConnectionNewAddress(ConnectionNewAddressFrame {
                source_account: Address::from_str("example.blah").unwrap()
            })
        );
        assert_eq!(iter.count(), 12);
    }

    #[test]
    fn it_saturates_max_money_frame_receive_max() {
        let mut buffer = BytesMut::new();
        buffer.put_var_uint(123); // stream_id
        buffer.put_var_octet_string(vec![
            // receive_max
            0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09,
        ]);
        buffer.put_var_uint(123); // total_received
        let frame = StreamMaxMoneyFrame::read_contents(&buffer).unwrap();
        assert_eq!(frame.receive_max, u64::MAX);
    }

    #[test]
    fn it_saturates_money_blocked_frame_send_max() {
        let mut buffer = BytesMut::new();
        buffer.put_var_uint(123); // stream_id
        buffer.put_var_octet_string(vec![
            // send_max
            0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09,
        ]);
        buffer.put_var_uint(123); // total_sent
        let frame = StreamMoneyBlockedFrame::read_contents(&buffer).unwrap();
        assert_eq!(frame.send_max, u64::MAX);
    }
}
