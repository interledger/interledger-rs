use byteorder::ReadBytesExt;
use bytes::{BufMut, Bytes, BytesMut};
use interledger_packet::{
    oer::{predict_var_octet_string, BufOerExt, MutBufOerExt},
    Fulfill, FulfillBuilder, ParseError, Prepare, PrepareBuilder,
};
use std::ops::Add;
use std::time::{Duration, SystemTime};

static ILDCP_DESTINATION: &'static [u8] = b"peer.config";
static PEER_PROTOCOL_FULFILLMENT: [u8; 32] = [0; 32];
static PEER_PROTOCOL_CONDITION: [u8; 32] = [
    102, 104, 122, 173, 248, 98, 189, 119, 108, 143, 193, 139, 142, 159, 142, 32, 8, 151, 20, 133,
    110, 226, 51, 179, 144, 42, 89, 29, 13, 95, 41, 37,
];
const ASSET_SCALE_LEN: usize = 1;

lazy_static! {
    static ref PEER_PROTOCOL_EXPIRY_DURATION: Duration = Duration::from_secs(60);
}

pub fn is_ildcp_request(prepare: &Prepare) -> bool {
    prepare.execution_condition() == PEER_PROTOCOL_CONDITION
        && prepare.destination() == ILDCP_DESTINATION
}

#[derive(Debug, Default)]
pub struct IldcpRequest {}

impl IldcpRequest {
    pub fn new() -> Self {
        IldcpRequest {}
    }

    pub fn to_prepare(&self) -> Prepare {
        PrepareBuilder {
            destination: ILDCP_DESTINATION,
            amount: 0,
            execution_condition: &PEER_PROTOCOL_CONDITION,
            expires_at: SystemTime::now().add(*PEER_PROTOCOL_EXPIRY_DURATION),
            data: &[],
        }
        .build()
    }
}

impl From<IldcpRequest> for Prepare {
    fn from(request: IldcpRequest) -> Self {
        request.to_prepare()
    }
}

#[derive(Debug, PartialEq)]
pub struct IldcpResponse {
    buffer: Bytes,
    asset_scale: u8,
    asset_code_offset: usize,
}

impl From<IldcpResponse> for Bytes {
    fn from(response: IldcpResponse) -> Self {
        response.buffer
    }
}

impl From<IldcpResponse> for Fulfill {
    fn from(response: IldcpResponse) -> Self {
        FulfillBuilder {
            fulfillment: &PEER_PROTOCOL_FULFILLMENT,
            data: &response.buffer[..],
        }
        .build()
    }
}

impl IldcpResponse {
    pub fn try_from(buffer: Bytes) -> Result<Self, ParseError> {
        let mut reader = &buffer[..];
        let buffer_len = reader.len();

        reader.skip_var_octet_string()?;
        let asset_scale = reader.read_u8()?;

        let asset_code_offset = buffer_len - reader.len();
        reader.skip_var_octet_string()?;

        Ok(IldcpResponse {
            buffer,
            asset_scale,
            asset_code_offset,
        })
    }

    pub fn client_address(&self) -> &[u8] {
        (&self.buffer[..]).peek_var_octet_string().unwrap()
    }

    pub fn asset_scale(&self) -> u8 {
        self.asset_scale
    }

    pub fn asset_code(&self) -> &[u8] {
        (&self.buffer[self.asset_code_offset..])
            .peek_var_octet_string()
            .unwrap()
    }
}

#[derive(Debug, PartialEq)]
pub struct IldcpResponseBuilder<'a> {
    pub client_address: &'a [u8],
    pub asset_scale: u8,
    pub asset_code: &'a str,
}

impl<'a> IldcpResponseBuilder<'a> {
    pub fn build(&self) -> IldcpResponse {
        let address_size = predict_var_octet_string(self.client_address.len());
        let asset_code_size = predict_var_octet_string(self.asset_code.len());
        let buf_size = ASSET_SCALE_LEN + address_size + asset_code_size;
        let mut buffer = BytesMut::with_capacity(buf_size);

        buffer.put_var_octet_string_length(self.client_address.len());
        buffer.put_slice(self.client_address);
        buffer.put_u8(self.asset_scale);
        buffer.put_var_octet_string_length(self.asset_code.len());
        buffer.put_slice(self.asset_code.as_bytes());

        IldcpResponse {
            buffer: buffer.freeze(),
            asset_scale: self.asset_scale,
            asset_code_offset: address_size + ASSET_SCALE_LEN,
        }
    }
}
