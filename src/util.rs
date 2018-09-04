use errors::ParseError;
use ilp_packet::IlpPacket;
use btp_packet::BtpPacket;

pub trait Serializable<T> {
    fn from_bytes(bytes: &[u8]) -> Result<T, ParseError>;

    fn to_bytes(&self) -> Result<Vec<u8>, ParseError>;
}

pub struct OutgoingRequestIdGenerator {
    next_id: u32
}

impl OutgoingRequestIdGenerator {
    pub fn new() -> OutgoingRequestIdGenerator {
        OutgoingRequestIdGenerator {
            next_id: 0
        }
    }

    pub fn get_next_id(&mut self) -> u32 {
        self.next_id += 1;
        self.next_id
    }
}

#[derive(Debug, PartialEq, Clone)]
pub enum IlpOrBtpPacket {
  Ilp(u32, IlpPacket),
  Btp(BtpPacket),
}

impl From<BtpPacket> for IlpOrBtpPacket {
  fn from(btp: BtpPacket) -> Self {
    IlpOrBtpPacket::Btp(btp)
  }
}