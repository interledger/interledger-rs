use byteorder::ReadBytesExt;
use bytes::{BufMut, BytesMut};
use core::borrow::Borrow;
use futures::future::err;
use interledger_packet::{
    oer::BufOerExt, Address, ErrorCode, Prepare, PrepareBuilder, RejectBuilder,
};
use interledger_service::*;
use log::debug;
use std::convert::TryFrom;
use std::marker::PhantomData;
use std::str;
use std::time::SystemTime;

/// A service that responds to the Echo Protocol.
/// Currently, this service only supports bidirectional mode (unidirectional mode is not supported yet).
/// The service doesn't shorten expiry as it expects the expiry to be shortened by another service
/// like `ExpiryShortenerService`.

/// The prefix that echo packets should have in its data section
const ECHO_PREFIX: &str = "ECHOECHOECHOECHO";
/// The length of the `ECHO_PREFIX`
const ECHO_PREFIX_LEN: usize = 16;

enum EchoPacketType {
    Request = 0,
    Response = 1,
}

#[derive(Clone)]
pub struct EchoService<I, S, A> {
    store: S,
    next: I,
    account_type: PhantomData<A>,
}

impl<I, S, A> EchoService<I, S, A>
where
    S: AddressStore,
    I: IncomingService<A>,
    A: Account,
{
    pub fn new(store: S, next: I) -> Self {
        EchoService {
            store,
            next,
            account_type: PhantomData,
        }
    }
}

impl<I, S, A> IncomingService<A> for EchoService<I, S, A>
where
    I: IncomingService<A>,
    S: AddressStore,
    A: Account,
{
    type Future = BoxedIlpFuture;

    fn handle_request(&mut self, mut request: IncomingRequest<A>) -> Self::Future {
        let ilp_address = self.store.get_ilp_address();
        let should_echo = request.prepare.destination() == ilp_address
            && request.prepare.data().starts_with(ECHO_PREFIX.as_bytes());
        if !should_echo {
            return Box::new(self.next.handle_request(request));
        }
        debug!("Responding to Echo protocol request: {:?}", request);

        // TODO Define EchoPacket struct and implement From<&p[u8]> for it

        let mut reader = request.prepare.data();

        // prefix is checked above, so skip prefix bytes.
        reader.skip(ECHO_PREFIX_LEN).unwrap();

        // check echo packet type
        let echo_packet_type = match reader.read_u8() {
            Ok(value) => value,
            Err(error) => {
                eprintln!("Could not read packet type: {:?}", error);
                return Box::new(err(RejectBuilder {
                    code: ErrorCode::F01_INVALID_PACKET,
                    message: b"Could not read echo packet type.",
                    triggered_by: Some(&ilp_address),
                    data: &[],
                }
                .build()));
            }
        };
        if echo_packet_type == EchoPacketType::Response as u8 {
            // if the echo packet type is Response, just pass it to the next service
            // so that the initiator could handle this packet
            return Box::new(self.next.handle_request(request));
        }
        if echo_packet_type != EchoPacketType::Request as u8 {
            eprintln!("The packet type is not acceptable: {}", echo_packet_type);
            return Box::new(err(RejectBuilder {
                code: ErrorCode::F01_INVALID_PACKET,
                message: format!(
                    "The echo packet type: {} is not acceptable.",
                    echo_packet_type
                )
                .as_bytes(),
                triggered_by: Some(&ilp_address),
                data: &[],
            }
            .build()));
        }

        // check source address
        let source_address = match reader.read_var_octet_string() {
            Ok(value) => match Address::try_from(value) {
                Ok(value) => value,
                Err(error) => {
                    eprintln!(
                        "Could not parse source address from echo packet: {:?}",
                        error
                    );
                    return Box::new(err(RejectBuilder {
                        code: ErrorCode::F01_INVALID_PACKET,
                        message: b"Could not parse source address from Echo packet",
                        triggered_by: Some(&ilp_address),
                        data: &[],
                    }
                    .build()));
                }
            },
            Err(error) => {
                eprintln!("Could not read source address: {:?}", error);
                return Box::new(err(RejectBuilder {
                    code: ErrorCode::F01_INVALID_PACKET,
                    message: b"Could not read source address.",
                    triggered_by: Some(&ilp_address),
                    data: &[],
                }
                .build()));
            }
        };

        let source_address = Address::try_from(source_address).unwrap();

        // create a new prepare packet to echo the prepare
        let execution_condition =
            <[u8; 32]>::try_from(request.prepare.execution_condition()).unwrap();
        request.prepare = EchoResponseBuilder {
            amount: request.prepare.amount(),
            expires_at: request.prepare.expires_at(),
            execution_condition: &execution_condition,
            destination: &source_address,
        }
        .build();

        Box::new(self.next.handle_request(request))
    }
}

#[cfg(test)]
use interledger_packet::{oer, oer::MutBufOerExt};
// This should be exported when we have a use for it outside of the tests
#[cfg(test)]
pub struct EchoRequestBuilder<'a> {
    pub amount: u64,
    pub expires_at: SystemTime,
    pub execution_condition: &'a [u8; 32],
    /// The ILP address that the initiator wants to Ping
    pub destination: &'a Address,
    /// The ILP address of the initiator
    pub source_address: &'a Address,
}

#[cfg(test)]
impl<'a> EchoRequestBuilder<'a> {
    pub fn build(&self) -> Prepare {
        let source_address_len = oer::predict_var_octet_string(self.source_address.len());
        let mut data_buffer = BytesMut::with_capacity(ECHO_PREFIX_LEN + 1 + source_address_len);
        data_buffer.put(ECHO_PREFIX.as_bytes());
        data_buffer.put_u8(EchoPacketType::Request as u8);
        data_buffer.put_var_octet_string(self.source_address.as_ref() as &[u8]);
        PrepareBuilder {
            amount: self.amount,
            expires_at: self.expires_at,
            execution_condition: self.execution_condition,
            destination: self.destination.clone(),
            data: data_buffer.borrow(),
        }
        .build()
    }
}

pub struct EchoResponseBuilder<'a> {
    pub amount: u64,
    pub expires_at: SystemTime,
    pub execution_condition: &'a [u8; 32],
    /// The ILP address of the initiator which is extracted from the data section of the echo request packet.
    pub destination: &'a Address,
}

impl<'a> EchoResponseBuilder<'a> {
    pub fn build(&self) -> Prepare {
        let mut data_buffer = BytesMut::with_capacity(ECHO_PREFIX_LEN + 1);
        data_buffer.put(ECHO_PREFIX.as_bytes());
        data_buffer.put_u8(EchoPacketType::Response as u8);
        PrepareBuilder {
            amount: self.amount,
            expires_at: self.expires_at,
            execution_condition: self.execution_condition,
            destination: self.destination.clone(),
            data: data_buffer.borrow(),
        }
        .build()
    }
}

#[cfg(test)]
mod echo_tests {
    use super::*;
    use futures::future::Future;
    use interledger_packet::{FulfillBuilder, PrepareBuilder};
    use interledger_service::incoming_service_fn;
    use lazy_static::lazy_static;
    use ring::digest::{digest, SHA256};
    use ring::rand::{SecureRandom, SystemRandom};
    use std::str::FromStr;
    use std::time::{Duration, SystemTime};

    lazy_static! {
        pub static ref ALICE: Username = Username::from_str("alice").unwrap();
        pub static ref EXAMPLE_ADDRESS: Address = Address::from_str("example.alice").unwrap();
    }

    #[derive(Clone)]
    struct TestStore(Address);

    impl AddressStore for TestStore {
        /// Saves the ILP Address in the store's memory and database
        fn set_ilp_address(
            &self,
            _ilp_address: Address,
        ) -> Box<dyn Future<Item = (), Error = ()> + Send> {
            unimplemented!()
        }

        fn clear_ilp_address(&self) -> Box<dyn Future<Item = (), Error = ()> + Send> {
            unimplemented!()
        }

        /// Get's the store's ilp address from memory
        fn get_ilp_address(&self) -> Address {
            self.0.clone()
        }
    }

    #[derive(Debug, Clone)]
    struct TestAccount(u64);

    impl Account for TestAccount {
        type AccountId = u64;
        fn id(&self) -> u64 {
            self.0
        }

        fn username(&self) -> &Username {
            &ALICE
        }

        fn asset_code(&self) -> &str {
            "XYZ"
        }

        // All connector accounts use asset scale = 9.
        fn asset_scale(&self) -> u8 {
            9
        }

        fn ilp_address(&self) -> &Address {
            &EXAMPLE_ADDRESS
        }
    }

    /// If the destination of the packet is not destined to the node's address,
    /// the node should not echo the packet.
    #[test]
    fn test_echo_packet_not_destined() {
        let amount = 1;
        let expires_at = SystemTime::now() + Duration::from_secs(30);
        let fulfillment = &get_random_fulfillment();
        let execution_condition = &get_hash_of(fulfillment);
        let destination = Address::from_str("example.recipient").unwrap();
        let data = b"ECHOECHOECHOECHO\x00\x11example.initiator";
        let node_address = Address::from_str("example.node").unwrap();
        let source_address = Address::from_str("example.initiator").unwrap();

        // setup service
        let handler = incoming_service_fn(|request| {
            assert_eq!(request.prepare.amount(), amount);
            assert_eq!(request.prepare.expires_at(), expires_at);
            assert_eq!(request.prepare.execution_condition(), execution_condition);
            assert_eq!(request.prepare.destination(), destination);
            assert_eq!(request.prepare.data(), &data[..]);
            Ok(FulfillBuilder {
                fulfillment: &fulfillment,
                data,
            }
            .build())
        });
        let mut echo_service = EchoService::new(TestStore(node_address), handler);

        // setup request
        let prepare = EchoRequestBuilder {
            amount,
            expires_at,
            execution_condition,
            destination: &destination,
            source_address: &source_address,
        }
        .build();
        let from = TestAccount(1);

        // test
        let result = echo_service
            .handle_request(IncomingRequest { prepare, from })
            .wait();
        assert!(result.is_ok());
    }

    /// Even if the destination of the packet is the node's address,
    /// packets that don't have a correct echo prefix will not be handled as echo packets.
    #[test]
    fn test_echo_packet_without_echo_prefix() {
        let amount = 1;
        let expires_at = SystemTime::now() + Duration::from_secs(30);
        let fulfillment = &get_random_fulfillment();
        let execution_condition = &get_hash_of(fulfillment);
        let destination = Address::from_str("example.recipient").unwrap();
        let data = b"ECHO";
        let node_address = Address::from_str("example.node").unwrap();

        // setup service
        let dest = destination.clone();
        let handler = incoming_service_fn(|request| {
            assert_eq!(request.prepare.amount(), amount);
            assert_eq!(request.prepare.expires_at(), expires_at);
            assert_eq!(request.prepare.execution_condition(), execution_condition);
            assert_eq!(request.prepare.destination(), dest);
            assert_eq!(request.prepare.data(), &data[..]);
            Ok(FulfillBuilder {
                fulfillment: &fulfillment,
                data: &[],
            }
            .build())
        });
        let mut echo_service = EchoService::new(TestStore(node_address), handler);

        // setup request
        let prepare = PrepareBuilder {
            amount,
            expires_at,
            execution_condition,
            destination,
            data,
        }
        .build();
        let from = TestAccount(1);

        // test
        let result = echo_service
            .handle_request(IncomingRequest { prepare, from })
            .wait();
        assert!(result.is_ok());
    }

    /// If the destination of the packet is the node's address and the echo packet type is
    /// request, the service will echo the packet modifying destination to the `source_address`.
    #[test]
    fn test_echo_packet() {
        let amount = 1;
        let expires_at = SystemTime::now() + Duration::from_secs(30);
        let fulfillment = &get_random_fulfillment();
        let execution_condition = &get_hash_of(fulfillment);
        let destination = Address::from_str("example.recipient").unwrap();
        let data = b"ECHOECHOECHOECHO\x01";
        let node_address = Address::from_str("example.recipient").unwrap();
        let source_address = Address::from_str("example.initiator").unwrap();

        // setup service
        let handler = incoming_service_fn(|request| {
            assert_eq!(request.prepare.amount(), amount);
            assert_eq!(request.prepare.expires_at(), expires_at);
            assert_eq!(request.prepare.execution_condition(), execution_condition);
            assert_eq!(request.prepare.destination(), source_address);
            assert_eq!(request.prepare.data(), &data[..]);
            Ok(FulfillBuilder {
                fulfillment: &fulfillment,
                data,
            }
            .build())
        });
        let mut echo_service = EchoService::new(TestStore(node_address), handler);

        // setup request
        let prepare = EchoRequestBuilder {
            amount,
            expires_at,
            execution_condition,
            destination: &destination,
            source_address: &source_address,
        }
        .build();
        let from = TestAccount(1);

        // test
        let result = echo_service
            .handle_request(IncomingRequest { prepare, from })
            .wait();
        assert!(result.is_ok());
    }

    /// If echo packet type is neither `1` nor `2`, the packet is considered to be malformed.
    #[test]
    fn test_invalid_echo_packet_type() {
        let amount = 1;
        let expires_at = SystemTime::now() + Duration::from_secs(30);
        let fulfillment = &get_random_fulfillment();
        let execution_condition = &get_hash_of(fulfillment);
        let destination = Address::from_str("example.recipient").unwrap();
        let data = b"ECHOECHOECHOECHO\x03\0x00";
        let node_address = Address::from_str("example.recipient").unwrap();

        // setup service
        let handler = incoming_service_fn(|_| {
            Err(RejectBuilder {
                code: ErrorCode::F01_INVALID_PACKET,
                message: &[],
                triggered_by: None,
                data: &[],
            }
            .build())
        });
        let mut echo_service = EchoService::new(TestStore(node_address), handler);

        // setup request
        let prepare = PrepareBuilder {
            amount,
            expires_at,
            execution_condition,
            destination,
            data,
        }
        .build();
        let from = TestAccount(1);

        // test
        let result = echo_service
            .handle_request(IncomingRequest { prepare, from })
            .wait();
        assert!(result.is_err());
    }

    /// Even if the destination of the packet is the node's address and the data starts with
    /// echo prefix correctly, `source_address` may be broken. This is the case.
    #[test]
    fn test_invalid_source_address() {
        let amount = 1;
        let expires_at = SystemTime::now() + Duration::from_secs(30);
        let fulfillment = &get_random_fulfillment();
        let execution_condition = &get_hash_of(fulfillment);
        let destination = Address::from_str("example.recipient").unwrap();
        let data = b"ECHOECHOECHOECHO\x00\x04abc";
        let node_address = Address::from_str("example.recipient").unwrap();

        // setup service
        let handler = incoming_service_fn(|_| {
            Err(RejectBuilder {
                code: ErrorCode::F01_INVALID_PACKET,
                message: &[],
                triggered_by: None,
                data: &[],
            }
            .build())
        });
        let mut echo_service = EchoService::new(TestStore(node_address), handler);

        // setup request
        let prepare = PrepareBuilder {
            amount,
            expires_at,
            execution_condition,
            destination,
            data,
        }
        .build();
        let from = TestAccount(1);

        // test
        let result = echo_service
            .handle_request(IncomingRequest { prepare, from })
            .wait();
        assert!(result.is_err());
    }

    fn get_random_fulfillment() -> [u8; 32] {
        let mut bytes: [u8; 32] = [0; 32];
        SystemRandom::new().fill(&mut bytes).unwrap();
        bytes
    }

    fn get_hash_of(preimage: &[u8]) -> [u8; 32] {
        let mut hash = [0; 32];
        hash.copy_from_slice(digest(&SHA256, preimage).as_ref());
        hash
    }
}
