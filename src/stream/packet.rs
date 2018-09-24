use super::crypto::{decrypt, encrypt};
use errors::ParseError;
use byteorder::{ReadBytesExt, WriteBytesExt};
use bytes::{Bytes, BytesMut};
use ilp::PacketType as IlpPacketType;
use num_bigint::BigUint;
use num_traits::cast::ToPrimitive;
use oer::{ReadOerExt, WriteOerExt};
use std::io::Cursor;

// TODO zero copy
const STREAM_VERSION: u8 = 1;

#[derive(Debug, PartialEq, Clone)]
pub struct StreamPacket {
  pub sequence: u64,
  pub ilp_packet_type: IlpPacketType,
  pub prepare_amount: u64,
  pub frames: Vec<Frame>,
}

impl StreamPacket {
  pub fn from_encrypted(shared_secret: Bytes, ciphertext: BytesMut) -> Result<Self, ParseError> {
    // TODO handle decryption failure
    let decrypted = decrypt(shared_secret, ciphertext)
      .map_err(|_err| {
        ParseError::InvalidPacket(String::from("Unable to decrypt packet"))
      })?;
    StreamPacket::from_bytes_unencrypted(&decrypted[..])
  }

  fn from_bytes_unencrypted(bytes: &[u8]) -> Result<Self, ParseError> {
    let mut reader = Cursor::new(bytes);
    let version = reader.read_u8()?;
    if version != STREAM_VERSION {
      return Err(ParseError::InvalidPacket(format!(
        "Unsupported STREAM version: {}",
        version
      )));
    }
    let ilp_packet_type_int = reader.read_u8()?;
    let ilp_packet_type = IlpPacketType::from(ilp_packet_type_int);
    if ilp_packet_type == IlpPacketType::Unknown {
      return Err(ParseError::InvalidPacket(format!(
        "Unknown ILP packet type: {}",
        ilp_packet_type_int
      )));
    }
    let sequence = reader
      .read_var_uint()?
      .to_u64()
      .ok_or(ParseError::InvalidPacket(String::from(
        "Packet sequence is greater than max u64",
      )))?;
    let prepare_amount = reader
      .read_var_uint()?
      .to_u64()
      .ok_or(ParseError::InvalidPacket(String::from(
        "Prepare amount is greater than max u64",
      )))?;
    let num_frames = reader
      .read_var_uint()?
      .to_u64()
      .ok_or(ParseError::InvalidPacket(String::from(
        "Num frames is greater than max u64",
      )))?;

    let mut frames: Vec<Frame> = Vec::new();
    for _i in 0..num_frames {
      let frame_type = reader.read_u8()?;
      let contents_vec = reader.read_var_octet_string()?;
      let mut contents = Cursor::new(contents_vec);
      let frame: Frame = match FrameType::from(frame_type) {
        FrameType::ConnectionClose => {
          Frame::ConnectionClose(ConnectionCloseFrame::read_contents(&mut contents)?)
        }
        FrameType::ConnectionNewAddress => {
          Frame::ConnectionNewAddress(ConnectionNewAddressFrame::read_contents(&mut contents)?)
        }
        FrameType::ConnectionAssetDetails => {
          Frame::ConnectionAssetDetails(ConnectionAssetDetailsFrame::read_contents(&mut contents)?)
        }
        FrameType::ConnectionMaxData => {
          Frame::ConnectionMaxData(ConnectionMaxDataFrame::read_contents(&mut contents)?)
        }
        FrameType::ConnectionDataBlocked => {
          Frame::ConnectionDataBlocked(ConnectionDataBlockedFrame::read_contents(&mut contents)?)
        }
        FrameType::ConnectionMaxStreamId => {
          Frame::ConnectionMaxStreamId(ConnectionMaxStreamIdFrame::read_contents(&mut contents)?)
        }
        FrameType::ConnectionStreamIdBlocked => Frame::ConnectionStreamIdBlocked(
          ConnectionStreamIdBlockedFrame::read_contents(&mut contents)?,
        ),
        FrameType::StreamClose => {
          Frame::StreamClose(StreamCloseFrame::read_contents(&mut contents)?)
        }
        FrameType::StreamMoney => {
          Frame::StreamMoney(StreamMoneyFrame::read_contents(&mut contents)?)
        }
        FrameType::StreamMaxMoney => {
          Frame::StreamMaxMoney(StreamMaxMoneyFrame::read_contents(&mut contents)?)
        }
        FrameType::StreamMoneyBlocked => {
          Frame::StreamMoneyBlocked(StreamMoneyBlockedFrame::read_contents(&mut contents)?)
        }
        FrameType::StreamData => Frame::StreamData(StreamDataFrame::read_contents(&mut contents)?),
        FrameType::StreamMaxData => {
          Frame::StreamMaxData(StreamMaxDataFrame::read_contents(&mut contents)?)
        }
        FrameType::StreamDataBlocked => {
          Frame::StreamDataBlocked(StreamDataBlockedFrame::read_contents(&mut contents)?)
        }
        FrameType::Unknown => {
          warn!(
            "Ignoring unknown frame of type {}: {:x?}",
            frame_type,
            contents.get_ref().as_slice()
          );
          Frame::Unknown
        }
      };
      frames.push(frame);
    }

    Ok(StreamPacket {
      sequence,
      ilp_packet_type,
      prepare_amount,
      frames,
    })
  }

  pub fn to_encrypted(&self, shared_secret: Bytes) -> Result<Bytes, ParseError> {
    let bytes = self.to_bytes_unencrypted()?;
    let ciphertext = encrypt(shared_secret, BytesMut::from(bytes)).to_vec();
    Ok(Bytes::from(ciphertext))
  }

  fn to_bytes_unencrypted(&self) -> Result<Vec<u8>, ParseError> {
    let mut writer = Vec::new();

    writer.write_u8(STREAM_VERSION)?;
    writer.write_u8(self.ilp_packet_type.clone() as u8)?;
    writer.write_var_uint(&BigUint::from(self.sequence))?;
    writer.write_var_uint(&BigUint::from(self.prepare_amount))?;
    writer.write_var_uint(&BigUint::from(self.frames.len()))?;

    for frame in &self.frames {
      let mut contents = Vec::new();
      match frame {
        Frame::ConnectionClose(ref frame) => {
          writer.write_u8(FrameType::ConnectionClose as u8)?;
          frame.write_contents(&mut contents)?;
        }
        Frame::ConnectionNewAddress(ref frame) => {
          writer.write_u8(FrameType::ConnectionNewAddress as u8)?;
          frame.write_contents(&mut contents)?;
        }
        Frame::ConnectionAssetDetails(ref frame) => {
          writer.write_u8(FrameType::ConnectionAssetDetails as u8)?;
          frame.write_contents(&mut contents)?;
        }
        Frame::ConnectionMaxData(ref frame) => {
          writer.write_u8(FrameType::ConnectionMaxData as u8)?;
          frame.write_contents(&mut contents)?;
        }
        Frame::ConnectionDataBlocked(ref frame) => {
          writer.write_u8(FrameType::ConnectionDataBlocked as u8)?;
          frame.write_contents(&mut contents)?;
        }
        Frame::ConnectionMaxStreamId(ref frame) => {
          writer.write_u8(FrameType::ConnectionMaxStreamId as u8)?;
          frame.write_contents(&mut contents)?;
        }
        Frame::ConnectionStreamIdBlocked(ref frame) => {
          writer.write_u8(FrameType::ConnectionStreamIdBlocked as u8)?;
          frame.write_contents(&mut contents)?;
        }
        Frame::StreamClose(ref frame) => {
          writer.write_u8(FrameType::StreamClose as u8)?;
          frame.write_contents(&mut contents)?;
        }
        Frame::StreamMoney(ref frame) => {
          writer.write_u8(FrameType::StreamMoney as u8)?;
          frame.write_contents(&mut contents)?;
        }
        Frame::StreamMaxMoney(ref frame) => {
          writer.write_u8(FrameType::StreamMaxMoney as u8)?;
          frame.write_contents(&mut contents)?;
        }
        Frame::StreamMoneyBlocked(ref frame) => {
          writer.write_u8(FrameType::StreamMoneyBlocked as u8)?;
          frame.write_contents(&mut contents)?;
        }
        Frame::StreamData(ref frame) => {
          writer.write_u8(FrameType::StreamData as u8)?;
          frame.write_contents(&mut contents)?;
        }
        Frame::StreamMaxData(ref frame) => {
          writer.write_u8(FrameType::StreamMaxData as u8)?;
          frame.write_contents(&mut contents)?;
        }
        Frame::StreamDataBlocked(ref frame) => {
          writer.write_u8(FrameType::StreamDataBlocked as u8)?;
          frame.write_contents(&mut contents)?;
        }
        Frame::Unknown => continue,
      }
      writer.write_var_octet_string(&contents)?;
    }
    Ok(writer)
  }
}

#[derive(Debug, PartialEq, Clone)]
pub enum Frame {
  ConnectionClose(ConnectionCloseFrame),
  ConnectionNewAddress(ConnectionNewAddressFrame),
  ConnectionAssetDetails(ConnectionAssetDetailsFrame),
  ConnectionMaxData(ConnectionMaxDataFrame),
  ConnectionDataBlocked(ConnectionDataBlockedFrame),
  ConnectionMaxStreamId(ConnectionMaxStreamIdFrame),
  ConnectionStreamIdBlocked(ConnectionStreamIdBlockedFrame),
  StreamClose(StreamCloseFrame),
  StreamMoney(StreamMoneyFrame),
  StreamMaxMoney(StreamMaxMoneyFrame),
  StreamMoneyBlocked(StreamMoneyBlockedFrame),
  StreamData(StreamDataFrame),
  StreamMaxData(StreamMaxDataFrame),
  StreamDataBlocked(StreamDataBlockedFrame),
  Unknown,
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

pub trait SerializableFrame: Sized {
  fn write_contents(&self, writer: &mut impl WriteOerExt) -> Result<(), ParseError>;

  fn read_contents(reader: &mut impl ReadOerExt) -> Result<Self, ParseError>;
}

#[derive(Debug, PartialEq, Clone)]
pub struct ConnectionCloseFrame {
  pub code: ErrorCode,
  pub message: String,
}

impl SerializableFrame for ConnectionCloseFrame {
  fn read_contents(reader: &mut impl ReadOerExt) -> Result<Self, ParseError> {
    let code = ErrorCode::from(reader.read_u8()?);
    let message = String::from_utf8(reader.read_var_octet_string()?)?;

    Ok(ConnectionCloseFrame { code, message })
  }

  fn write_contents(&self, writer: &mut impl WriteOerExt) -> Result<(), ParseError> {
    writer.write_u8(self.code.clone() as u8)?;
    writer.write_var_octet_string(self.message.as_bytes())?;
    Ok(())
  }
}

#[derive(Debug, PartialEq, Clone)]
pub struct ConnectionNewAddressFrame {
  pub source_account: String,
}

impl SerializableFrame for ConnectionNewAddressFrame {
  fn read_contents(reader: &mut impl ReadOerExt) -> Result<Self, ParseError> {
    let source_account = String::from_utf8(reader.read_var_octet_string()?)?;

    Ok(ConnectionNewAddressFrame { source_account })
  }

  fn write_contents(&self, writer: &mut impl WriteOerExt) -> Result<(), ParseError> {
    writer.write_var_octet_string(self.source_account.as_bytes())?;
    Ok(())
  }
}

#[derive(Debug, PartialEq, Clone)]
pub struct ConnectionAssetDetailsFrame {
  pub source_asset_code: String,
  pub source_asset_scale: u8,
}

impl SerializableFrame for ConnectionAssetDetailsFrame {
  fn read_contents(reader: &mut impl ReadOerExt) -> Result<Self, ParseError> {
    let source_asset_code = String::from_utf8(reader.read_var_octet_string()?)?;
    let source_asset_scale = reader.read_u8()?;

    Ok(ConnectionAssetDetailsFrame {
      source_asset_scale,
      source_asset_code,
    })
  }

  fn write_contents(&self, writer: &mut impl WriteOerExt) -> Result<(), ParseError> {
    writer.write_var_octet_string(self.source_asset_code.as_bytes())?;
    writer.write_u8(self.source_asset_scale)?;
    Ok(())
  }
}

#[derive(Debug, PartialEq, Clone)]
pub struct ConnectionMaxDataFrame {
  pub max_offset: BigUint,
}

impl SerializableFrame for ConnectionMaxDataFrame {
  fn read_contents(reader: &mut impl ReadOerExt) -> Result<Self, ParseError> {
    let max_offset = reader.read_var_uint()?;

    Ok(ConnectionMaxDataFrame { max_offset })
  }

  fn write_contents(&self, writer: &mut impl WriteOerExt) -> Result<(), ParseError> {
    writer.write_var_uint(&self.max_offset)?;
    Ok(())
  }
}

#[derive(Debug, PartialEq, Clone)]
pub struct ConnectionDataBlockedFrame {
  pub max_offset: BigUint,
}

impl SerializableFrame for ConnectionDataBlockedFrame {
  fn read_contents(reader: &mut impl ReadOerExt) -> Result<Self, ParseError> {
    let max_offset = reader.read_var_uint()?;

    Ok(ConnectionDataBlockedFrame { max_offset })
  }

  fn write_contents(&self, writer: &mut impl WriteOerExt) -> Result<(), ParseError> {
    writer.write_var_uint(&self.max_offset)?;
    Ok(())
  }
}

#[derive(Debug, PartialEq, Clone)]
pub struct ConnectionMaxStreamIdFrame {
  pub max_stream_id: BigUint,
}

impl SerializableFrame for ConnectionMaxStreamIdFrame {
  fn read_contents(reader: &mut impl ReadOerExt) -> Result<Self, ParseError> {
    let max_stream_id = reader.read_var_uint()?;

    Ok(ConnectionMaxStreamIdFrame { max_stream_id })
  }

  fn write_contents(&self, writer: &mut impl WriteOerExt) -> Result<(), ParseError> {
    writer.write_var_uint(&self.max_stream_id)?;
    Ok(())
  }
}

#[derive(Debug, PartialEq, Clone)]
pub struct ConnectionStreamIdBlockedFrame {
  pub max_stream_id: BigUint,
}

impl SerializableFrame for ConnectionStreamIdBlockedFrame {
  fn read_contents(reader: &mut impl ReadOerExt) -> Result<Self, ParseError> {
    let max_stream_id = reader.read_var_uint()?;

    Ok(ConnectionStreamIdBlockedFrame { max_stream_id })
  }

  fn write_contents(&self, writer: &mut impl WriteOerExt) -> Result<(), ParseError> {
    writer.write_var_uint(&self.max_stream_id)?;
    Ok(())
  }
}

#[derive(Debug, PartialEq, Clone)]
pub struct StreamCloseFrame {
  pub stream_id: BigUint,
  pub code: ErrorCode,
  pub message: String,
}

impl SerializableFrame for StreamCloseFrame {
  fn read_contents(reader: &mut impl ReadOerExt) -> Result<Self, ParseError> {
    let stream_id = reader.read_var_uint()?;
    let code = ErrorCode::from(reader.read_u8()?);
    let message = String::from_utf8(reader.read_var_octet_string()?)?;

    Ok(StreamCloseFrame {
      stream_id,
      code,
      message,
    })
  }

  fn write_contents(&self, writer: &mut impl WriteOerExt) -> Result<(), ParseError> {
    writer.write_var_uint(&self.stream_id)?;
    writer.write_u8(self.code.clone() as u8)?;
    writer.write_var_octet_string(self.message.as_bytes())?;
    Ok(())
  }
}

#[derive(Debug, PartialEq, Clone)]
pub struct StreamMoneyFrame {
  pub stream_id: BigUint,
  pub shares: BigUint,
}

impl SerializableFrame for StreamMoneyFrame {
  fn read_contents(reader: &mut impl ReadOerExt) -> Result<Self, ParseError> {
    let stream_id = reader.read_var_uint()?;
    let shares = reader.read_var_uint()?;

    Ok(StreamMoneyFrame { stream_id, shares })
  }

  fn write_contents(&self, writer: &mut impl WriteOerExt) -> Result<(), ParseError> {
    writer.write_var_uint(&self.stream_id)?;
    writer.write_var_uint(&self.shares)?;
    Ok(())
  }
}

#[derive(Debug, PartialEq, Clone)]
pub struct StreamMaxMoneyFrame {
  pub stream_id: BigUint,
  pub receive_max: BigUint,
  pub total_received: BigUint,
}

impl SerializableFrame for StreamMaxMoneyFrame {
  fn read_contents(reader: &mut impl ReadOerExt) -> Result<Self, ParseError> {
    let stream_id = reader.read_var_uint()?;
    let receive_max = reader.read_var_uint()?;
    let total_received = reader.read_var_uint()?;

    Ok(StreamMaxMoneyFrame {
      stream_id,
      receive_max,
      total_received,
    })
  }

  fn write_contents(&self, writer: &mut impl WriteOerExt) -> Result<(), ParseError> {
    writer.write_var_uint(&self.stream_id)?;
    writer.write_var_uint(&self.receive_max)?;
    writer.write_var_uint(&self.total_received)?;
    Ok(())
  }
}

#[derive(Debug, PartialEq, Clone)]
pub struct StreamMoneyBlockedFrame {
  pub stream_id: BigUint,
  pub send_max: BigUint,
  pub total_sent: BigUint,
}

impl SerializableFrame for StreamMoneyBlockedFrame {
  fn read_contents(reader: &mut impl ReadOerExt) -> Result<Self, ParseError> {
    let stream_id = reader.read_var_uint()?;
    let send_max = reader.read_var_uint()?;
    let total_sent = reader.read_var_uint()?;

    Ok(StreamMoneyBlockedFrame {
      stream_id,
      send_max,
      total_sent,
    })
  }

  fn write_contents(&self, writer: &mut impl WriteOerExt) -> Result<(), ParseError> {
    writer.write_var_uint(&self.stream_id)?;
    writer.write_var_uint(&self.send_max)?;
    writer.write_var_uint(&self.total_sent)?;
    Ok(())
  }
}

#[derive(Debug, PartialEq, Clone)]
pub struct StreamDataFrame {
  pub stream_id: BigUint,
  pub offset: BigUint,
  pub data: Vec<u8>,
}

impl SerializableFrame for StreamDataFrame {
  fn read_contents(reader: &mut impl ReadOerExt) -> Result<Self, ParseError> {
    let stream_id = reader.read_var_uint()?;
    let offset = reader.read_var_uint()?;
    let data = reader.read_var_octet_string()?;

    Ok(StreamDataFrame {
      stream_id,
      offset,
      data,
    })
  }

  fn write_contents(&self, writer: &mut impl WriteOerExt) -> Result<(), ParseError> {
    writer.write_var_uint(&self.stream_id)?;
    writer.write_var_uint(&self.offset)?;
    writer.write_var_octet_string(&self.data)?;
    Ok(())
  }
}

#[derive(Debug, PartialEq, Clone)]
pub struct StreamMaxDataFrame {
  pub stream_id: BigUint,
  pub max_offset: BigUint,
}

impl SerializableFrame for StreamMaxDataFrame {
  fn read_contents(reader: &mut impl ReadOerExt) -> Result<Self, ParseError> {
    let stream_id = reader.read_var_uint()?;
    let max_offset = reader.read_var_uint()?;

    Ok(StreamMaxDataFrame {
      stream_id,
      max_offset,
    })
  }

  fn write_contents(&self, writer: &mut impl WriteOerExt) -> Result<(), ParseError> {
    writer.write_var_uint(&self.stream_id)?;
    writer.write_var_uint(&self.max_offset)?;
    Ok(())
  }
}

#[derive(Debug, PartialEq, Clone)]
pub struct StreamDataBlockedFrame {
  pub stream_id: BigUint,
  pub max_offset: BigUint,
}

impl SerializableFrame for StreamDataBlockedFrame {
  fn read_contents(reader: &mut impl ReadOerExt) -> Result<Self, ParseError> {
    let stream_id = reader.read_var_uint()?;
    let max_offset = reader.read_var_uint()?;

    Ok(StreamDataBlockedFrame {
      stream_id,
      max_offset,
    })
  }

  fn write_contents(&self, writer: &mut impl WriteOerExt) -> Result<(), ParseError> {
    writer.write_var_uint(&self.stream_id)?;
    writer.write_var_uint(&self.max_offset)?;
    Ok(())
  }
}

#[cfg(test)]
mod serialization {
  use super::*;

  lazy_static! {
    static ref PACKET: StreamPacket = StreamPacket {
      sequence: 1,
      ilp_packet_type: IlpPacketType::from(12),
      prepare_amount: 99,
      frames: vec![
        Frame::ConnectionClose(ConnectionCloseFrame {
          code: ErrorCode::NoError,
          message: String::from("oop")
        }),
        Frame::ConnectionNewAddress(ConnectionNewAddressFrame {
          source_account: String::from("example.blah")
        }),
        Frame::ConnectionMaxData(ConnectionMaxDataFrame {
          max_offset: BigUint::from(1000 as u64)
        }),
        Frame::ConnectionDataBlocked(ConnectionDataBlockedFrame {
          max_offset: BigUint::from(2000 as u64)
        }),
        Frame::ConnectionMaxStreamId(ConnectionMaxStreamIdFrame {
          max_stream_id: BigUint::from(3000 as u64)
        }),
        Frame::ConnectionStreamIdBlocked(ConnectionStreamIdBlockedFrame {
          max_stream_id: BigUint::from(4000 as u64)
        }),
        Frame::ConnectionAssetDetails(ConnectionAssetDetailsFrame {
          source_asset_code: String::from("XYZ"),
          source_asset_scale: 9
        }),
        Frame::StreamClose(StreamCloseFrame {
          stream_id: BigUint::from(76 as u64),
          code: ErrorCode::InternalError,
          message: String::from("blah")
        }),
        Frame::StreamMoney(StreamMoneyFrame {
          stream_id: BigUint::from(88 as u64),
          shares: BigUint::from(99 as u64)
        }),
        Frame::StreamMaxMoney(StreamMaxMoneyFrame {
          stream_id: BigUint::from(11 as u64),
          receive_max: BigUint::from(987 as u64),
          total_received: BigUint::from(500 as u64)
        }),
        Frame::StreamMoneyBlocked(StreamMoneyBlockedFrame {
          stream_id: BigUint::from(66 as u64),
          send_max: BigUint::from(20000 as u64),
          total_sent: BigUint::from(6000 as u64)
        }),
        Frame::StreamData(StreamDataFrame {
          stream_id: BigUint::from(34 as u64),
          offset: BigUint::from(9000 as u64),
          data: String::from("hello").as_bytes().to_vec(),
        }),
        Frame::StreamMaxData(StreamMaxDataFrame {
          stream_id: BigUint::from(35 as u64),
          max_offset: BigUint::from(8766 as u64)
        }),
        Frame::StreamDataBlocked(StreamDataBlockedFrame {
          stream_id: BigUint::from(888 as u64),
          max_offset: BigUint::from(44444 as u64)
        }),
      ]
    };
    static ref SERIALIZED: Vec<u8> = vec![
      1, 12, 1, 1, 1, 99, 1, 14, 1, 5, 1, 3, 111, 111, 112, 2, 13, 12, 101, 120, 97, 109, 112, 108,
      101, 46, 98, 108, 97, 104, 3, 3, 2, 3, 232, 4, 3, 2, 7, 208, 5, 3, 2, 11, 184, 6, 3, 2, 15,
      160, 7, 5, 3, 88, 89, 90, 9, 16, 8, 1, 76, 2, 4, 98, 108, 97, 104, 17, 4, 1, 88, 1, 99, 18,
      8, 1, 11, 2, 3, 219, 2, 1, 244, 19, 8, 1, 66, 2, 78, 32, 2, 23, 112, 20, 11, 1, 34, 2, 35,
      40, 5, 104, 101, 108, 108, 111, 21, 5, 1, 35, 2, 34, 62, 22, 6, 2, 3, 120, 2, 173, 156
    ];
  }

  #[test]
  fn it_serializes_to_same_as_javascript() {
    assert_eq!(PACKET.to_bytes_unencrypted().unwrap(), *SERIALIZED);
  }

  #[test]
  fn it_deserializes_from_javascript() {
    assert_eq!(
      StreamPacket::from_bytes_unencrypted(&SERIALIZED[..]).unwrap(),
      *PACKET
    );
  }
}
