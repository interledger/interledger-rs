use byteorder::{WriteBytesExt, ReadBytesExt, BigEndian};
use oer::{WriteOerExt, ReadOerExt};
use std::io::Cursor;
use std::io::prelude::*;
use ilp::PacketType as IlpPacketType;
use super::errors::ParseError;

// TODO zero copy

#[derive(Debug, PartialEq, Clone)]
pub struct StreamPacket {
  sequence: u64,
  ilp_packet_type: IlpPacketType,
  prepareAmount: u64,
  frames: Vec<Frame>
}

#[derive(Debug, PartialEq, Clone)]
pub enum Frame {
  ConnectionClose(ConnectionCloseFrame),
  Unknown
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

// pub trait Serializable : Sized {
//   fn from_bytes(bytes: &[u8]) -> Result<Self, ParseError>;

//   fn write_to(&self, &mut Vec<u8>) -> Result<(), ParseError>;

//   fn to_bytes(&self) -> Vec<u8> {
//     let mut vec: Vec<u8> = Vec::new();
//     self.write_to(&mut vec);
//     vec
//   }

//   fn len(&self) -> usize {
//     // TODO more efficient implementation
//     self.to_bytes().len()
//   }
// }

pub trait SerializableFrame : Sized {
  fn write_contents(&self, writer: &mut impl WriteOerExt)-> Result<(), ParseError> ;

  fn read_contents(reader: &mut impl ReadOerExt) -> Result<Self, ParseError>;
}

#[derive(Debug, PartialEq, Clone)]
pub struct ConnectionCloseFrame {
  code: ErrorCode,
  name: String,
  message: String,
}

impl SerializableFrame for ConnectionCloseFrame {
  fn read_contents(reader: &mut impl ReadOerExt) -> Result<Self, ParseError> {
    let code = ErrorCode::from(reader.read_u8()?);
    let name = String::from_utf8(reader.read_var_octet_string()?)?;
    let message = String::from_utf8(reader.read_var_octet_string()?)?;

    Ok(ConnectionCloseFrame {
      code,
      name,
      message,
    })
  }

  fn write_contents(&self, writer: &mut impl WriteOerExt) -> Result<(), ParseError> {
    writer.write_u8(self.code.clone() as u8)?;
    writer.write_var_octet_string(self.name.as_bytes())?;
    writer.write_var_octet_string(self.message.as_bytes())?;
    Ok(())
  }
}
