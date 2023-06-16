use bytes::{Buf, BufMut, Bytes};
use interledger_packet::{
    hex::HexString,
    oer::{self, BufOerExt, MutBufOerExt},
    Address, AddressError, Fulfill, FulfillBuilder, OerError, Prepare, PrepareBuilder,
};
use once_cell::sync::Lazy;
use std::{
    convert::{TryFrom, TryInto},
    fmt::{self, Debug},
    str::{self, FromStr},
    time::{Duration, SystemTime},
};
use tracing::error;

pub const PEER_PROTOCOL_FULFILLMENT: [u8; 32] = [0; 32];
pub const PEER_PROTOCOL_CONDITION: [u8; 32] = [
    102, 104, 122, 173, 248, 98, 189, 119, 108, 143, 193, 139, 142, 159, 142, 32, 8, 151, 20, 133,
    110, 226, 51, 179, 144, 42, 89, 29, 13, 95, 41, 37,
];
const PEER_PROTOCOL_EXPIRY_DURATION: u64 = 60000;
const FLAG_OPTIONAL: u8 = 0x80;
const FLAG_TRANSITIVE: u8 = 0x40;
const FLAG_PARTIAL: u8 = 0x20;
const FLAG_UTF8: u8 = 0x10;

const ROUTING_TABLE_ID_LEN: usize = 16;

/// All epochs are u32
const EPOCH_LEN: usize = 4;

const AUTH_LEN: usize = 32;

pub static CCP_RESPONSE: Lazy<Fulfill> = Lazy::new(|| {
    FulfillBuilder {
        fulfillment: &PEER_PROTOCOL_FULFILLMENT,
        data: &[],
    }
    .build()
});
pub static CCP_CONTROL_DESTINATION: Lazy<Address> =
    Lazy::new(|| Address::from_str("peer.route.control").unwrap());
pub static CCP_UPDATE_DESTINATION: Lazy<Address> =
    Lazy::new(|| Address::from_str("peer.route.update").unwrap());

#[derive(Debug)]
pub enum CcpPacketError {
    UnexpectedMode(u8),
    PacketExpired,
    UnexpectedDestination(Address),
    UnexpectedCondition([u8; 32]),
    Oer(OerError),
    Utf8Conversion,
    AddresssInvalid(AddressError),
}

impl fmt::Display for CcpPacketError {
    fn fmt(&self, fmt: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CcpPacketError::UnexpectedMode(val) => {
                write!(fmt, "Invalid Packet: Unexpected Mode found: {}", val)
            }
            CcpPacketError::PacketExpired => write!(fmt, "Invalid Packet: Packet expired"),
            CcpPacketError::UnexpectedDestination(address) => {
                write!(
                    fmt,
                    "Invalid Packet: Packet is not a CCP message. Destination: {}",
                    address
                )
            }
            CcpPacketError::UnexpectedCondition(c) => {
                write!(fmt, "Invalid Packet: Wrong condition: {:?}", HexString(c))
            }
            CcpPacketError::Oer(err) => write!(fmt, "Invalid Packet: {}", err),
            CcpPacketError::Utf8Conversion => write!(fmt, "Unable to convert data to utf-8"),
            CcpPacketError::AddresssInvalid(err) => write!(fmt, "Address Invalid {:?}", err),
        }
    }
}

impl From<OerError> for CcpPacketError {
    fn from(err: OerError) -> Self {
        CcpPacketError::Oer(err)
    }
}

impl From<std::str::Utf8Error> for CcpPacketError {
    fn from(_err: std::str::Utf8Error) -> Self {
        CcpPacketError::Utf8Conversion
    }
}

impl From<AddressError> for CcpPacketError {
    fn from(err: AddressError) -> Self {
        CcpPacketError::AddresssInvalid(err)
    }
}

/// CCP Packet mode used in Route Control Requests of the CCP protocol.
/// Idle: Account does not wish to receive more routes
/// Sync: Account wishes to receive routes
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum Mode {
    Idle = 0,
    Sync = 1,
}

impl Mode {
    /// Length of Mode on the wire (bytes)
    const LEN: usize = 1;
}

impl TryFrom<u8> for Mode {
    type Error = CcpPacketError;

    fn try_from(val: u8) -> Result<Self, Self::Error> {
        match val {
            0 => Ok(Mode::Idle),
            1 => Ok(Mode::Sync),
            _ => Err(CcpPacketError::UnexpectedMode(val)),
        }
    }
}

/// A request that ask the receiver node to transition to Idle or Sync mode.
/// If the mode is Idle, the receiver of the request will stop broadcasting routes to the sender.
/// If the mode is Sync, the receiver will start broadcasting routes to that account.
#[derive(Clone, PartialEq, Eq)]
pub struct RouteControlRequest {
    pub mode: Mode,
    pub last_known_routing_table_id: [u8; ROUTING_TABLE_ID_LEN],
    pub last_known_epoch: u32,
    pub features: Vec<String>,
}

impl Debug for RouteControlRequest {
    fn fmt(&self, fmt: &mut fmt::Formatter<'_>) -> Result<(), fmt::Error> {
        fmt.debug_struct("RouteControlRequest")
            .field("mode", &self.mode)
            .field(
                "last_known_routing_table_id",
                &HexString(&self.last_known_routing_table_id),
            )
            .field("last_known_epoch", &self.last_known_epoch)
            .field("features", &self.features)
            .finish()
    }
}

impl TryFrom<&Prepare> for RouteControlRequest {
    type Error = CcpPacketError;

    fn try_from(prepare: &Prepare) -> Result<Self, Self::Error> {
        if prepare.expires_at() < SystemTime::now() {
            return Err(CcpPacketError::PacketExpired);
        }
        RouteControlRequest::try_from_without_expiry(prepare)
    }
}

impl RouteControlRequest {
    pub(crate) fn try_from_without_expiry(prepare: &Prepare) -> Result<Self, CcpPacketError> {
        let destination = prepare.destination();
        if destination != *CCP_CONTROL_DESTINATION {
            return Err(CcpPacketError::UnexpectedDestination(destination));
        }

        if prepare.execution_condition() != PEER_PROTOCOL_CONDITION {
            error!("Unexpected condition: {:x?}", prepare.execution_condition());
            return Err(CcpPacketError::UnexpectedCondition(
                prepare
                    .execution_condition()
                    .try_into()
                    .expect("Always return a length of 32"),
            ));
        }

        Self::try_from_data(prepare.data())
    }

    #[cfg(any(fuzzing, test))]
    pub fn fuzz_from_prepare_data(data: &[u8]) {
        if let Ok(s) = Self::try_from_data(data) {
            let prepare = s.to_prepare();
            let roundtripped = Self::try_from_without_expiry(&prepare).unwrap();
            assert_eq!(s, roundtripped);
        }
    }

    fn try_from_data(mut data: &[u8]) -> Result<Self, CcpPacketError> {
        const MIN_LEN: usize = Mode::LEN
            + ROUTING_TABLE_ID_LEN
            // u32
            + EPOCH_LEN
            // zero features would be the minimum
            + oer::MIN_VARUINT_LEN;

        if data.remaining() < MIN_LEN {
            return Err(OerError::UnexpectedEof.into());
        }
        let mode = Mode::try_from(data.get_u8())?;
        let mut last_known_routing_table_id = [0; ROUTING_TABLE_ID_LEN];
        data.copy_to_slice(&mut last_known_routing_table_id);
        let last_known_epoch = data.get_u32();

        // TODO: see discussion for Route::try_from(&mut &[u8])
        let num_features = data.read_var_uint()?;
        let mut features: Vec<String> = Vec::new();
        for _i in 0..num_features {
            features.push(str::from_utf8(data.read_var_octet_string()?)?.to_owned());
        }

        Ok(RouteControlRequest {
            mode,
            last_known_routing_table_id,
            last_known_epoch,
            features,
        })
    }

    pub fn to_prepare(&self) -> Prepare {
        let mut data = Vec::new();

        data.put_u8(self.mode as u8);
        data.put_slice(&self.last_known_routing_table_id);
        data.put_u32(self.last_known_epoch);
        data.put_var_uint(self.features.len() as u64);
        for feature in self.features.iter() {
            data.put_var_octet_string(feature.as_bytes());
        }

        PrepareBuilder {
            destination: CCP_CONTROL_DESTINATION.clone(),
            amount: 0,
            expires_at: SystemTime::now() + Duration::from_millis(PEER_PROTOCOL_EXPIRY_DURATION),
            execution_condition: &PEER_PROTOCOL_CONDITION,
            data: &data[..],
        }
        .build()
    }
}

impl From<RouteControlRequest> for Prepare {
    fn from(request: RouteControlRequest) -> Self {
        request.to_prepare()
    }
}

#[derive(Clone, PartialEq, Debug)]
pub(crate) struct RouteProp {
    pub(crate) is_optional: bool,
    pub(crate) is_transitive: bool,
    pub(crate) is_partial: bool,
    pub(crate) id: u16,
    pub(crate) is_utf8: bool,
    pub(crate) value: Bytes,
}

impl TryFrom<&mut &[u8]> for RouteProp {
    type Error = CcpPacketError;

    // Note this takes a mutable ref to the slice so that it advances the cursor in the original slice
    fn try_from(data: &mut &[u8]) -> Result<Self, Self::Error> {
        const FLAG_LEN: usize = 1;
        const ID_LEN: usize = 2;

        const MIN_LEN: usize = FLAG_LEN + ID_LEN + oer::EMPTY_VARLEN_OCTETS_LEN;

        if data.remaining() < MIN_LEN {
            return Err(OerError::UnexpectedEof.into());
        }

        let meta = data.get_u8();

        let is_optional = meta & FLAG_OPTIONAL != 0;
        let is_transitive = meta & FLAG_TRANSITIVE != 0;
        let is_partial = meta & FLAG_PARTIAL != 0;
        let is_utf8 = meta & FLAG_UTF8 != 0;

        let id = data.get_u16();
        let value = Bytes::copy_from_slice(data.read_var_octet_string()?);

        Ok(RouteProp {
            is_optional,
            is_transitive,
            is_partial,
            id,
            is_utf8,
            value,
        })
    }
}

impl RouteProp {
    pub fn write_to<B>(&self, buf: &mut B)
    where
        B: BufMut,
    {
        let mut meta: u8 = 0;
        if self.is_optional {
            meta |= FLAG_OPTIONAL;
        }
        if self.is_partial {
            meta |= FLAG_PARTIAL;
        }
        if self.is_transitive {
            meta |= FLAG_TRANSITIVE;
        }
        if self.is_utf8 {
            meta |= FLAG_UTF8;
        }

        buf.put_u8(meta);
        buf.put_u16(self.id);
        buf.put_var_octet_string(&self.value[..]);
    }
}

#[derive(Clone, PartialEq)]
pub(crate) struct Route {
    // TODO switch this to use the Address type so we don't need separate parsing logic when implementing Debug
    pub(crate) prefix: String,
    pub(crate) path: Vec<String>,
    pub(crate) auth: [u8; AUTH_LEN],
    pub(crate) props: Vec<RouteProp>,
}

impl Debug for Route {
    fn fmt(&self, fmt: &mut fmt::Formatter) -> Result<(), fmt::Error> {
        fmt.debug_struct("Route")
            .field("prefix", &self.prefix)
            .field("path", &self.path)
            .field("auth", &HexString(&self.auth))
            .field("props", &self.props)
            .finish()
    }
}

impl TryFrom<&mut &[u8]> for Route {
    type Error = CcpPacketError;

    // Note this takes a mutable ref to the slice so that it advances the cursor in the original slice
    fn try_from(data: &mut &[u8]) -> Result<Self, Self::Error> {
        let prefix = str::from_utf8(data.read_var_octet_string()?)?.to_string();

        // FIXME: this is prone to pre-allocating a lot (see fuzzed test cases), like every
        // instance of reading a `length` as var_uint and following it up with a preallocation in
        // this crate. Other crates such as interledger-stream gets away from this by having an
        // iterator which just iterates the values starting from the specific offset and `length`
        // is checked against the number of elements iterator found. interledger-btp use just the
        // default growing of the vec (unideal) and check against trailing bytes (missing here).
        //
        // Using the unideal version (growing vec) for now.
        let path_len = data.read_var_uint()? as usize;
        let mut path: Vec<String> = Vec::new();
        for _i in 0..path_len {
            path.push(str::from_utf8(data.read_var_octet_string()?)?.to_string());
        }
        if data.remaining() < AUTH_LEN {
            return Err(OerError::UnexpectedEof.into());
        }
        let mut auth = [0u8; AUTH_LEN];
        data.copy_to_slice(&mut auth);

        // TODO: see discussion above
        let prop_len = data.read_var_uint()? as usize;
        let mut props = Vec::new();
        for _i in 0..prop_len {
            // For some reason we need to cast `data to `&mut &[u8]` again, otherwise
            // error[E0382]: use of moved value: `data`
            // fn try_from(data: &mut &[u8]) -> Result<Self, Self::Error> {
            // move occurs because `data` has type `&mut &[u8]`, which does not implement the `Copy` trait
            // props.push(RouteProp::try_from(data /* as &mut &[u8] */)?);
            //                                ^^^^ value moved here, in previous iteration of loop
            props.push(RouteProp::try_from(data as &mut &[u8])?);
        }

        Ok(Route {
            prefix,
            path,
            auth,
            props,
        })
    }
}

impl Route {
    pub fn write_to<B>(&self, buf: &mut B)
    where
        B: BufMut,
    {
        buf.put_var_octet_string(self.prefix.as_bytes());
        buf.put_var_uint(self.path.len() as u64);
        for address in self.path.iter() {
            buf.put_var_octet_string(address.as_bytes());
        }
        buf.put(&self.auth[..]);
        buf.put_var_uint(self.props.len() as u64);
        for prop in self.props.iter() {
            prop.write_to(buf);
        }
    }
}

#[derive(Clone, PartialEq)]
pub struct RouteUpdateRequest {
    pub(crate) routing_table_id: [u8; ROUTING_TABLE_ID_LEN],
    pub(crate) current_epoch_index: u32,
    pub(crate) from_epoch_index: u32,
    pub(crate) to_epoch_index: u32,
    pub(crate) hold_down_time: u32,
    pub(crate) speaker: Address,
    pub(crate) new_routes: Vec<Route>,
    pub(crate) withdrawn_routes: Vec<String>,
}

impl Debug for RouteUpdateRequest {
    fn fmt(&self, fmt: &mut fmt::Formatter) -> Result<(), fmt::Error> {
        fmt.debug_struct("RouteUpdateRequest")
            .field("routing_table_id", &HexString(&self.routing_table_id))
            .field("current_epoch_index", &self.current_epoch_index)
            .field("from_epoch_index", &self.from_epoch_index)
            .field("to_epoch_index", &self.to_epoch_index)
            .field("hold_down_time", &self.hold_down_time)
            .field("speaker", &self.speaker)
            .field("new_routes", &self.new_routes)
            .field("withdrawn_routes", &self.withdrawn_routes)
            .finish()
    }
}

impl TryFrom<&Prepare> for RouteUpdateRequest {
    type Error = CcpPacketError;

    fn try_from(prepare: &Prepare) -> Result<Self, Self::Error> {
        if prepare.expires_at() < SystemTime::now() {
            return Err(CcpPacketError::PacketExpired);
        }
        RouteUpdateRequest::try_from_without_expiry(prepare)
    }
}

impl RouteUpdateRequest {
    pub(crate) fn try_from_without_expiry(prepare: &Prepare) -> Result<Self, CcpPacketError> {
        let destination = prepare.destination();
        if destination != *CCP_UPDATE_DESTINATION {
            return Err(CcpPacketError::UnexpectedDestination(destination));
        }

        if prepare.execution_condition() != PEER_PROTOCOL_CONDITION {
            error!("Unexpected condition: {:x?}", prepare.execution_condition());
            return Err(CcpPacketError::UnexpectedCondition(
                prepare
                    .execution_condition()
                    .try_into()
                    .expect("Always return a length of 32"),
            ));
        }

        Self::try_from_data(prepare.data())
    }

    #[cfg(any(fuzzing, test))]
    pub fn fuzz_from_prepare_data(data: &[u8]) {
        if let Ok(s) = Self::try_from_data(data) {
            let prepare = s.to_prepare();
            let roundtripped = Self::try_from_without_expiry(&prepare).unwrap();
            assert_eq!(s, roundtripped);
        }
    }

    fn try_from_data(mut data: &[u8]) -> Result<Self, CcpPacketError> {
        const HOLD_DOWN_TIME_LEN: usize = 4;

        const MIN_LEN: usize = ROUTING_TABLE_ID_LEN
            // current_epoch_index, from_epoch_index, to_epoch_index
            + EPOCH_LEN * 3
            // hold_down_time
            + HOLD_DOWN_TIME_LEN
            + Address::MIN_LEN
            + interledger_packet::oer::MIN_VARUINT_LEN
            + interledger_packet::oer::MIN_VARUINT_LEN;

        if data.remaining() < MIN_LEN {
            return Err(OerError::UnexpectedEof.into());
        }

        let mut routing_table_id = [0u8; ROUTING_TABLE_ID_LEN];
        data.copy_to_slice(&mut routing_table_id);
        let current_epoch_index = data.get_u32();
        let from_epoch_index = data.get_u32();
        let to_epoch_index = data.get_u32();
        let hold_down_time = data.get_u32();
        let speaker = Address::try_from(data.read_var_octet_string()?)?;

        // TODO: see discussion for Route::try_from(&mut &[u8])
        let new_routes_len = data.read_var_uint()? as usize;
        let mut new_routes: Vec<Route> = Vec::new();
        for _i in 0..new_routes_len {
            new_routes.push(Route::try_from(&mut data)?);
        }

        // TODO: see discussion for Route::try_from(&mut &[u8])
        let withdrawn_routes_len = data.read_var_uint()? as usize;
        let mut withdrawn_routes = Vec::new();
        for _i in 0..withdrawn_routes_len {
            withdrawn_routes.push(str::from_utf8(data.read_var_octet_string()?)?.to_string());
        }

        Ok(RouteUpdateRequest {
            routing_table_id,
            current_epoch_index,
            from_epoch_index,
            to_epoch_index,
            hold_down_time,
            speaker,
            new_routes,
            withdrawn_routes,
        })
    }

    pub fn to_prepare(&self) -> Prepare {
        let mut data = Vec::new();
        data.put(&self.routing_table_id[..]);
        data.put_u32(self.current_epoch_index);
        data.put_u32(self.from_epoch_index);
        data.put_u32(self.to_epoch_index);
        data.put_u32(self.hold_down_time);
        data.put_var_octet_string(self.speaker.as_bytes());
        data.put_var_uint(self.new_routes.len() as u64);
        for route in self.new_routes.iter() {
            route.write_to(&mut data);
        }
        data.put_var_uint(self.withdrawn_routes.len() as u64);
        for route in self.withdrawn_routes.iter() {
            data.put_var_octet_string(route.as_bytes());
        }

        PrepareBuilder {
            destination: CCP_UPDATE_DESTINATION.clone(),
            amount: 0,
            expires_at: SystemTime::now() + Duration::from_millis(PEER_PROTOCOL_EXPIRY_DURATION),
            execution_condition: &PEER_PROTOCOL_CONDITION,
            data: &data[..],
        }
        .build()
    }
}

impl From<RouteUpdateRequest> for Prepare {
    fn from(request: RouteUpdateRequest) -> Self {
        request.to_prepare()
    }
}

#[cfg(test)]
mod route_control_request {
    use super::*;
    use crate::fixtures::*;
    use bytes::BytesMut;
    use hex_literal::hex;

    #[test]
    fn deserialize_too_short() {
        let prepare = Prepare::try_from(BytesMut::from(CONTROL_REQUEST_SERIALIZED)).unwrap();
        let data = prepare.data();
        for len in 0..data.len() {
            RouteControlRequest::try_from_data(&data[..len]).unwrap_err();
        }
    }

    #[test]
    fn deserialize() {
        let prepare = Prepare::try_from(BytesMut::from(CONTROL_REQUEST_SERIALIZED)).unwrap();
        let request = RouteControlRequest::try_from_without_expiry(&prepare).unwrap();
        assert_eq!(request, *CONTROL_REQUEST);
    }

    #[test]
    fn serialize() {
        let prepare = CONTROL_REQUEST.to_prepare();
        let test_prepare = Prepare::try_from(BytesMut::from(CONTROL_REQUEST_SERIALIZED)).unwrap();
        // Note this doesn't compare the serialized values directly because we aren't using mock timers so the expires at dates come out different
        assert_eq!(prepare.data(), test_prepare.data(),);
    }

    #[test]
    fn errors_with_wrong_destination() {
        let prepare = Prepare::try_from(BytesMut::from(&hex!("0c6c0000000000000000323031353036313630303031303030303066687aadf862bd776c8fc18b8e9f8e20089714856ee233b3902a591d0d5f292512706565722e726f7574652e636f6e74726f6b1f0170d1a134a0df4f47964f6e19e2ab379000000020010203666f6f03626172")[..])).unwrap();
        let result = RouteControlRequest::try_from_without_expiry(&prepare);
        assert_eq!(
            result.unwrap_err().to_string(),
            "Invalid Packet: Packet is not a CCP message. Destination: peer.route.controk"
        );
    }

    #[test]
    fn errors_with_wrong_condition() {
        let prepare = Prepare::try_from(BytesMut::from(&hex!("0c6c0000000000000000323031353036313630303031303030303066687aadf862bd776c8fc18b8e9f8e21089714856ee233b3902a591d0d5f292512706565722e726f7574652e636f6e74726f6c1f0170d1a134a0df4f47964f6e19e2ab379000000020010203666f6f03626172")[..])).unwrap();
        let result = RouteControlRequest::try_from_without_expiry(&prepare);
        assert_eq!(
            result.unwrap_err().to_string(),
            "Invalid Packet: Wrong condition: 66687aadf862bd776c8fc18b8e9f8e21089714856ee233b3902a591d0d5f2925"
        );
    }

    #[test]
    fn errors_with_expired_packet() {
        let prepare = Prepare::try_from(BytesMut::from(&hex!("0c6c0000000000000000323031343036313630303031303030303066687aadf862bd776c8fc18b8e9f8e20089714856ee233b3902a591d0d5f292512706565722e726f7574652e636f6e74726f6c1f0170d1a134a0df4f47964f6e19e2ab379000000020010203666f6f03626172")[..])).unwrap();
        let result = RouteControlRequest::try_from(&prepare);
        assert_eq!(
            result.unwrap_err().to_string(),
            "Invalid Packet: Packet expired"
        );
    }
}

#[cfg(test)]
mod route_update_request {
    use super::*;
    use crate::fixtures::*;
    use bytes::BytesMut;
    use hex_literal::hex;

    #[test]
    fn deserialize_too_short() {
        let prepare = Prepare::try_from(BytesMut::from(UPDATE_REQUEST_COMPLEX_SERIALIZED)).unwrap();
        let data = prepare.data();
        for len in 0..data.len() {
            RouteUpdateRequest::try_from_data(&data[..len]).unwrap_err();
        }
    }

    #[test]
    fn deserialize() {
        let prepare = Prepare::try_from(BytesMut::from(UPDATE_REQUEST_SIMPLE_SERIALIZED)).unwrap();
        let request = RouteUpdateRequest::try_from_without_expiry(&prepare).unwrap();
        assert_eq!(request, *UPDATE_REQUEST_SIMPLE);
    }

    #[test]
    fn serialize() {
        let prepare = UPDATE_REQUEST_SIMPLE.to_prepare();
        let test_prepare =
            Prepare::try_from(BytesMut::from(UPDATE_REQUEST_SIMPLE_SERIALIZED)).unwrap();
        assert_eq!(prepare.data(), test_prepare.data());
    }

    #[test]
    fn deserialize_complex_too_short() {
        let prepare = Prepare::try_from(BytesMut::from(UPDATE_REQUEST_COMPLEX_SERIALIZED)).unwrap();
        let data = prepare.data();
        for len in 0..data.len() {
            RouteUpdateRequest::try_from_data(&data[..len]).unwrap_err();
        }
    }

    #[test]
    fn deserialize_complex() {
        let prepare = Prepare::try_from(BytesMut::from(UPDATE_REQUEST_COMPLEX_SERIALIZED)).unwrap();
        let request = RouteUpdateRequest::try_from_without_expiry(&prepare).unwrap();
        assert_eq!(request, *UPDATE_REQUEST_COMPLEX);
    }

    #[test]
    fn serialize_complex() {
        let prepare = UPDATE_REQUEST_COMPLEX.to_prepare();
        let test_prepare =
            Prepare::try_from(BytesMut::from(UPDATE_REQUEST_COMPLEX_SERIALIZED)).unwrap();
        assert_eq!(prepare.data(), test_prepare.data());
    }

    #[test]
    fn errors_with_wrong_destination() {
        let prepare = Prepare::try_from(BytesMut::from(&hex!("0c7e0000000000000000323031353036313630303031303030303066687aadf862bd776c8fc18b8e9f8e20089714856ee233b3902a591d0d5f292511706565722e726f7574652e7570646174643221e55f8eabcd4e979ab9bf0ff00a224c000000340000003400000034000075300d6578616d706c652e616c69636501000100")[..])).unwrap();
        let result = RouteUpdateRequest::try_from_without_expiry(&prepare);
        assert_eq!(
            result.unwrap_err().to_string(),
            "Invalid Packet: Packet is not a CCP message. Destination: peer.route.updatd"
        );
    }

    #[test]
    fn errors_with_wrong_condition() {
        let prepare = Prepare::try_from(BytesMut::from(&hex!("0c7e0000000000000000323031353036313630303031303030303066687aadf862bd776c8fd18b8e9f8e20089714856ee233b3902a591d0d5f292511706565722e726f7574652e7570646174653221e55f8eabcd4e979ab9bf0ff00a224c000000340000003400000034000075300d6578616d706c652e616c69636501000100")[..])).unwrap();
        let result = RouteUpdateRequest::try_from_without_expiry(&prepare);
        assert_eq!(result.unwrap_err().to_string(), "Invalid Packet: Wrong condition: 66687aadf862bd776c8fd18b8e9f8e20089714856ee233b3902a591d0d5f2925");
    }

    #[test]
    fn errors_with_expired_packet() {
        let prepare = Prepare::try_from(BytesMut::from(&hex!("0c7e0000000000000000323031343036313630303031303030303066687aadf862bd776c8fc18b8e9f8e20089714856ee233b3902a591d0d5f292511706565722e726f7574652e7570646174653221e55f8eabcd4e979ab9bf0ff00a224c000000340000003400000034000075300d6578616d706c652e616c69636501000100")[..])).unwrap();
        let result = RouteUpdateRequest::try_from(&prepare);
        assert_eq!(
            result.unwrap_err().to_string(),
            "Invalid Packet: Packet expired"
        );
    }

    #[test]
    fn route_prop() {
        let prop = RouteProp {
            is_optional: true,
            is_partial: false,
            is_utf8: false,
            is_transitive: true,
            value: Bytes::from("test test test"),
            id: 9999,
        };

        let mut serialized = Vec::new();
        prop.write_to(&mut serialized);

        assert_eq!(prop, RouteProp::try_from(&mut &serialized[..]).unwrap());
    }

    #[test]
    fn route() {
        let route = Route {
            prefix: "example.some-prefix-for-alice".to_string(),
            path: vec![
                "example.some-other-connector".to_string(),
                "example.and-another-one".to_string(),
                "example.some-prefix-for-alice".to_string(),
            ],
            auth: [9; 32],
            props: vec![
                RouteProp {
                    is_optional: false,
                    is_partial: true,
                    is_utf8: false,
                    is_transitive: true,
                    value: Bytes::from("prop1"),
                    id: 0,
                },
                RouteProp {
                    is_optional: false,
                    is_partial: false,
                    is_utf8: false,
                    is_transitive: false,
                    value: Bytes::from("prop2"),
                    id: 7777,
                },
            ],
        };

        let mut serialized = Vec::new();
        route.write_to(&mut serialized);

        assert_eq!(route, Route::try_from(&mut &serialized[..]).unwrap());
    }
}
