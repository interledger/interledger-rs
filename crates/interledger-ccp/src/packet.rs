use byteorder::{BigEndian, ReadBytesExt};
use bytes::{BufMut, Bytes};
use hex;
use interledger_packet::{
    oer::{BufOerExt, MutBufOerExt},
    Fulfill, FulfillBuilder, ParseError, Prepare, PrepareBuilder,
};
use std::{
    io::Read,
    str,
    time::{Duration, SystemTime},
};

pub const CCP_CONTROL_DESTINATION: &[u8] = b"peer.route.control";
pub const CCP_UPDATE_DESTINATION: &[u8] = b"peer.route.update";
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

lazy_static! {
    pub static ref CCP_RESPONSE: Fulfill = FulfillBuilder {
        fulfillment: &PEER_PROTOCOL_FULFILLMENT,
        data: &[],
    }
    .build();
}

#[derive(Clone, Copy, PartialEq, Debug)]
#[repr(u8)]
pub enum Mode {
    Idle = 0,
    Sync = 1,
}

impl Mode {
    pub fn try_from(val: u8) -> Result<Self, ParseError> {
        match val {
            0 => Ok(Mode::Idle),
            1 => Ok(Mode::Sync),
            _ => Err(ParseError::InvalidPacket(format!(
                "Unexpected mode: {}",
                val
            ))),
        }
    }
}

#[derive(Clone, PartialEq, Debug)]
pub struct RouteControlRequest {
    pub(crate) mode: Mode,
    // TODO change debug to format this as hex
    pub(crate) last_known_routing_table_id: [u8; 16],
    pub(crate) last_known_epoch: u32,
    pub(crate) features: Vec<String>,
}

impl RouteControlRequest {
    pub fn try_from(prepare: &Prepare) -> Result<Self, ParseError> {
        if prepare.expires_at() < SystemTime::now() {
            return Err(ParseError::InvalidPacket("Packet expired".to_string()));
        }
        RouteControlRequest::try_from_without_expiry(prepare)
    }

    pub(crate) fn try_from_without_expiry(prepare: &Prepare) -> Result<Self, ParseError> {
        if prepare.destination() != CCP_CONTROL_DESTINATION {
            return Err(ParseError::InvalidPacket(format!(
                "Packet is not a CCP message. Destination: {}",
                str::from_utf8(prepare.destination()).unwrap_or("<not utf8>")
            )));
        }

        if prepare.execution_condition() != PEER_PROTOCOL_CONDITION {
            error!("Unexpected condition: {:x?}", prepare.execution_condition());
            return Err(ParseError::InvalidPacket(format!(
                "Wrong condition: {}",
                hex::encode(prepare.execution_condition()),
            )));
        }

        let mut data = prepare.data();

        let mode = Mode::try_from(data.read_u8()?)?;
        let mut last_known_routing_table_id: [u8; 16] = [0; 16];
        data.read_exact(&mut last_known_routing_table_id)?;
        let last_known_epoch = data.read_u32::<BigEndian>()?;
        let num_features = data.read_var_uint()?;
        let mut features: Vec<String> = Vec::with_capacity(num_features as usize);
        for _i in 0..num_features {
            features.push(String::from_utf8(data.read_var_octet_string()?.to_vec())?);
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
        data.put_u32_be(self.last_known_epoch);
        data.put_var_uint(self.features.len() as u64);
        for feature in self.features.iter() {
            data.put_var_octet_string(feature.as_bytes().to_vec());
        }

        PrepareBuilder {
            destination: CCP_CONTROL_DESTINATION,
            amount: 0,
            expires_at: SystemTime::now() + Duration::from_millis(PEER_PROTOCOL_EXPIRY_DURATION),
            execution_condition: &PEER_PROTOCOL_CONDITION,
            data: &data[..],
        }
        .build()
    }
}

#[derive(Clone, PartialEq, Debug)]
pub struct RouteProp {
    pub(crate) is_optional: bool,
    pub(crate) is_transitive: bool,
    pub(crate) is_partial: bool,
    pub(crate) id: u16,
    pub(crate) is_utf8: bool,
    pub(crate) value: Bytes,
}

impl RouteProp {
    // Note this takes a mutable ref to the slice so that it advances the cursor in the original slice
    pub fn try_from(data: &mut &[u8]) -> Result<Self, ParseError> {
        let meta = data.read_u8()?;

        let is_optional = meta & FLAG_OPTIONAL != 0;
        let is_transitive = meta & FLAG_TRANSITIVE != 0;
        let is_partial = meta & FLAG_PARTIAL != 0;
        let is_utf8 = meta & FLAG_UTF8 != 0;

        let id = data.read_u16::<BigEndian>()?;
        let value = Bytes::from(data.read_var_octet_string()?);

        Ok(RouteProp {
            is_optional,
            is_transitive,
            is_partial,
            id,
            is_utf8,
            value,
        })
    }

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
        buf.put_u16_be(self.id);
        buf.put_var_octet_string(&self.value[..]);
    }
}

#[derive(Clone, PartialEq, Debug)]
pub struct Route {
    pub(crate) prefix: Bytes,
    pub(crate) path: Vec<Bytes>,
    pub(crate) auth: [u8; 32],
    pub(crate) props: Vec<RouteProp>,
}

impl Route {
    // Note this takes a mutable ref to the slice so that it advances the cursor in the original slice
    pub fn try_from(data: &mut &[u8]) -> Result<Self, ParseError> {
        let prefix = Bytes::from(data.read_var_octet_string()?);
        let path_len = data.read_var_uint()? as usize;
        let mut path = Vec::with_capacity(path_len);
        for _i in 0..path_len {
            path.push(Bytes::from(data.read_var_octet_string()?));
        }
        let mut auth: [u8; 32] = [0; 32];
        data.read_exact(&mut auth)?;

        let prop_len = data.read_var_uint()? as usize;
        let mut props = Vec::with_capacity(prop_len);
        for _i in 0..prop_len {
            props.push(RouteProp::try_from(data)?);
        }

        Ok(Route {
            prefix,
            path,
            auth,
            props,
        })
    }

    pub fn write_to<B>(&self, buf: &mut B)
    where
        B: BufMut,
    {
        buf.put_var_octet_string(&self.prefix[..]);
        buf.put_var_uint(self.path.len() as u64);
        for address in self.path.iter() {
            buf.put_var_octet_string(&address[..]);
        }
        buf.put(&self.auth[..]);
        buf.put_var_uint(self.props.len() as u64);
        for prop in self.props.iter() {
            prop.write_to(buf);
        }
    }
}

#[derive(Clone, PartialEq, Debug)]
pub struct RouteUpdateRequest {
    pub(crate) routing_table_id: [u8; 16],
    pub(crate) current_epoch_index: u32,
    pub(crate) from_epoch_index: u32,
    pub(crate) to_epoch_index: u32,
    pub(crate) hold_down_time: u32,
    pub(crate) speaker: Bytes,
    pub(crate) new_routes: Vec<Route>,
    pub(crate) withdrawn_routes: Vec<Bytes>,
}

impl RouteUpdateRequest {
    pub fn try_from(prepare: &Prepare) -> Result<Self, ParseError> {
        if prepare.expires_at() < SystemTime::now() {
            return Err(ParseError::InvalidPacket("Packet expired".to_string()));
        }
        RouteUpdateRequest::try_from_without_expiry(prepare)
    }

    pub(crate) fn try_from_without_expiry(prepare: &Prepare) -> Result<Self, ParseError> {
        if prepare.destination() != CCP_UPDATE_DESTINATION {
            return Err(ParseError::InvalidPacket(format!(
                "Packet is not a CCP message. Destination: {}",
                str::from_utf8(prepare.destination()).unwrap_or("<not utf8>")
            )));
        }

        if prepare.execution_condition() != PEER_PROTOCOL_CONDITION {
            error!("Unexpected condition: {:x?}", prepare.execution_condition());
            return Err(ParseError::InvalidPacket(format!(
                "Wrong condition: {}",
                hex::encode(prepare.execution_condition()),
            )));
        }

        let mut data = prepare.data();
        let mut routing_table_id: [u8; 16] = [0; 16];
        data.read_exact(&mut routing_table_id)?;
        let current_epoch_index = data.read_u32::<BigEndian>()?;
        let from_epoch_index = data.read_u32::<BigEndian>()?;
        let to_epoch_index = data.read_u32::<BigEndian>()?;
        let hold_down_time = data.read_u32::<BigEndian>()?;
        let speaker = Bytes::from(data.read_var_octet_string()?);
        let new_routes_len = data.read_var_uint()? as usize;
        let mut new_routes: Vec<Route> = Vec::with_capacity(new_routes_len);
        for _i in 0..new_routes_len {
            new_routes.push(Route::try_from(&mut data)?);
        }
        let withdrawn_routes_len = data.read_var_uint()? as usize;
        let mut withdrawn_routes = Vec::with_capacity(withdrawn_routes_len);
        for _i in 0..withdrawn_routes_len {
            withdrawn_routes.push(Bytes::from(data.read_var_octet_string()?));
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
        data.put_u32_be(self.current_epoch_index);
        data.put_u32_be(self.from_epoch_index);
        data.put_u32_be(self.to_epoch_index);
        data.put_u32_be(self.hold_down_time);
        data.put_var_octet_string(&self.speaker[..]);
        data.put_var_uint(self.new_routes.len() as u64);
        for route in self.new_routes.iter() {
            route.write_to(&mut data);
        }
        data.put_var_uint(self.withdrawn_routes.len() as u64);
        for route in self.withdrawn_routes.iter() {
            data.put_var_octet_string(&route[..]);
        }

        PrepareBuilder {
            destination: CCP_UPDATE_DESTINATION,
            amount: 0,
            expires_at: SystemTime::now() + Duration::from_millis(PEER_PROTOCOL_EXPIRY_DURATION),
            execution_condition: &PEER_PROTOCOL_CONDITION,
            data: &data[..],
        }
        .build()
    }
}

#[cfg(test)]
mod route_control_request {
    use super::*;
    use crate::fixtures::*;
    use bytes::BytesMut;

    #[test]
    fn deserialize() {
        let prepare = Prepare::try_from(BytesMut::from(&CONTROL_REQUEST_SERIALIZED[..])).unwrap();
        let request = RouteControlRequest::try_from_without_expiry(&prepare).unwrap();
        assert_eq!(request, *CONTROL_REQUEST);
    }

    #[test]
    fn serialize() {
        let prepare = CONTROL_REQUEST.to_prepare();
        let test_prepare =
            Prepare::try_from(BytesMut::from(&CONTROL_REQUEST_SERIALIZED[..])).unwrap();
        // Note this doesn't compare the serialized values directly because we aren't using mock timers so the expires at dates come out different
        assert_eq!(prepare.data(), test_prepare.data(),);
    }

    #[test]
    fn errors_with_wrong_destination() {
        let prepare = Prepare::try_from(BytesMut::from(hex::decode("0c6c0000000000000000323031353036313630303031303030303066687aadf862bd776c8fc18b8e9f8e20089714856ee233b3902a591d0d5f292512706565722e726f7574652e636f6e74726f6b1f0170d1a134a0df4f47964f6e19e2ab379000000020010203666f6f03626172").unwrap())).unwrap();
        let result = RouteControlRequest::try_from_without_expiry(&prepare);
        assert!(result.is_err());
        assert_eq!(
            result.unwrap_err().to_string(),
            "Invalid Packet Packet is not a CCP message. Destination: peer.route.controk"
        );
    }

    #[test]
    fn errors_with_wrong_condition() {
        let prepare = Prepare::try_from(BytesMut::from(hex::decode("0c6c0000000000000000323031353036313630303031303030303066687aadf862bd776c8fc18b8e9f8e21089714856ee233b3902a591d0d5f292512706565722e726f7574652e636f6e74726f6c1f0170d1a134a0df4f47964f6e19e2ab379000000020010203666f6f03626172").unwrap())).unwrap();
        let result = RouteControlRequest::try_from_without_expiry(&prepare);
        assert!(result.is_err());
        assert_eq!(
            result.unwrap_err().to_string(),
            "Invalid Packet Wrong condition: 66687aadf862bd776c8fc18b8e9f8e21089714856ee233b3902a591d0d5f2925"
        );
    }

    #[test]
    fn errors_with_expired_packet() {
        let prepare = Prepare::try_from(BytesMut::from(hex::decode("0c6c0000000000000000323031343036313630303031303030303066687aadf862bd776c8fc18b8e9f8e20089714856ee233b3902a591d0d5f292512706565722e726f7574652e636f6e74726f6c1f0170d1a134a0df4f47964f6e19e2ab379000000020010203666f6f03626172").unwrap())).unwrap();
        let result = RouteControlRequest::try_from(&prepare);
        assert!(result.is_err());
        assert_eq!(
            result.unwrap_err().to_string(),
            "Invalid Packet Packet expired"
        );
    }
}

#[cfg(test)]
mod route_update_request {
    use super::*;
    use crate::fixtures::*;
    use bytes::BytesMut;

    #[test]
    fn deserialize() {
        let prepare =
            Prepare::try_from(BytesMut::from(&UPDATE_REQUEST_SIMPLE_SERIALIZED[..])).unwrap();
        let request = RouteUpdateRequest::try_from_without_expiry(&prepare).unwrap();
        assert_eq!(request, *UPDATE_REQUEST_SIMPLE);
    }

    #[test]
    fn serialize() {
        let prepare = UPDATE_REQUEST_SIMPLE.to_prepare();
        let test_prepare =
            Prepare::try_from(BytesMut::from(&UPDATE_REQUEST_SIMPLE_SERIALIZED[..])).unwrap();
        assert_eq!(prepare.data(), test_prepare.data());
    }

    #[test]
    fn deserialize_complex() {
        let prepare =
            Prepare::try_from(BytesMut::from(&UPDATE_REQUEST_COMPLEX_SERIALIZED[..])).unwrap();
        let request = RouteUpdateRequest::try_from_without_expiry(&prepare).unwrap();
        assert_eq!(request, *UPDATE_REQUEST_COMPLEX);
    }

    #[test]
    fn serialize_complex() {
        let prepare = UPDATE_REQUEST_COMPLEX.to_prepare();
        let test_prepare =
            Prepare::try_from(BytesMut::from(&UPDATE_REQUEST_COMPLEX_SERIALIZED[..])).unwrap();
        assert_eq!(prepare.data(), test_prepare.data());
    }

    #[test]
    fn errors_with_wrong_destination() {
        let prepare = Prepare::try_from(BytesMut::from(hex::decode("0c7e0000000000000000323031353036313630303031303030303066687aadf862bd776c8fc18b8e9f8e20089714856ee233b3902a591d0d5f292511706565722e726f7574652e7570646174643221e55f8eabcd4e979ab9bf0ff00a224c000000340000003400000034000075300d6578616d706c652e616c69636501000100").unwrap())).unwrap();
        let result = RouteUpdateRequest::try_from_without_expiry(&prepare);
        assert!(result.is_err());
        assert_eq!(
            result.unwrap_err().to_string(),
            "Invalid Packet Packet is not a CCP message. Destination: peer.route.updatd"
        );
    }

    #[test]
    fn errors_with_wrong_condition() {
        let prepare = Prepare::try_from(BytesMut::from(hex::decode("0c7e0000000000000000323031353036313630303031303030303066687aadf862bd776c8fd18b8e9f8e20089714856ee233b3902a591d0d5f292511706565722e726f7574652e7570646174653221e55f8eabcd4e979ab9bf0ff00a224c000000340000003400000034000075300d6578616d706c652e616c69636501000100").unwrap())).unwrap();
        let result = RouteUpdateRequest::try_from_without_expiry(&prepare);
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().to_string(), "Invalid Packet Wrong condition: 66687aadf862bd776c8fd18b8e9f8e20089714856ee233b3902a591d0d5f2925");
    }

    #[test]
    fn errors_with_expired_packet() {
        let prepare = Prepare::try_from(BytesMut::from(hex::decode("0c7e0000000000000000323031343036313630303031303030303066687aadf862bd776c8fc18b8e9f8e20089714856ee233b3902a591d0d5f292511706565722e726f7574652e7570646174653221e55f8eabcd4e979ab9bf0ff00a224c000000340000003400000034000075300d6578616d706c652e616c69636501000100").unwrap())).unwrap();
        let result = RouteUpdateRequest::try_from(&prepare);
        assert!(result.is_err());
        assert_eq!(
            result.unwrap_err().to_string(),
            "Invalid Packet Packet expired"
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
            prefix: Bytes::from("example.some-prefix-for-alice"),
            path: vec![
                Bytes::from("example.some-other-connector"),
                Bytes::from("example.and-another-one"),
                Bytes::from("example.some-prefix-for-alice"),
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
