use super::crypto::*;
use super::packet::*;
use async_trait::async_trait;
use base64;
use bytes::{Bytes, BytesMut};
use chrono::{DateTime, Utc};
use futures::channel::mpsc::UnboundedSender;
use hex;
use interledger_packet::{
    Address, ErrorCode, Fulfill, FulfillBuilder, PacketType as IlpPacketType, Prepare, Reject,
    RejectBuilder,
};
use interledger_service::{Account, IlpResult, OutgoingRequest, OutgoingService, Username};
use log::debug;
use serde::{Deserialize, Serialize};
use std::marker::PhantomData;
use std::time::SystemTime;
use uuid::Uuid;

// Note we are using the same magic bytes as the Javascript
// implementation but this is not strictly necessary. These
// magic bytes need to be the same for the server that creates the
// STREAM details for a given packet and for the server that fulfills
// it, but in the vast majority of cases those two servers will be
// running the same STREAM implementation so it doesn't matter what
// this string is.
const STREAM_SERVER_SECRET_GENERATOR: &[u8] = b"ilp_stream_shared_secret";

/// A STREAM connection generator that creates `destination_account` and `shared_secret` values
/// based on a single root secret.
///
/// This can be reused across multiple STREAM connections so that a single receiver can
/// accept incoming packets for multiple connections.
#[derive(Clone)]
pub struct ConnectionGenerator {
    secret_generator: Bytes,
}

impl ConnectionGenerator {
    pub fn new(server_secret: Bytes) -> Self {
        assert_eq!(server_secret.len(), 32, "Server secret must be 32 bytes");
        ConnectionGenerator {
            secret_generator: Bytes::from(
                &hmac_sha256(&server_secret[..], STREAM_SERVER_SECRET_GENERATOR)[..],
            ),
        }
    }

    /// Generate the STREAM parameters for the given ILP address and the configured server secret.
    ///
    /// The `destination_account` is generated such that the `shared_secret` can be re-derived
    /// from a Prepare packet's destination and the same server secret.
    pub fn generate_address_and_secret(&self, base_address: &Address) -> (Address, [u8; 32]) {
        let token = base64::encode_config(&generate_token(), base64::URL_SAFE_NO_PAD);
        // Note the shared secret is generated from the base64-encoded version of the token,
        // rather than from the unencoded bytes
        let shared_secret = hmac_sha256(&self.secret_generator[..], &token.as_bytes()[..]);
        // Note that the unwrap here is safe because we know the base_address
        // is valid and adding base64-url characters will always be valid
        let destination_account = base_address.with_suffix(&token.as_ref()).unwrap();

        debug!("Generated address: {}", destination_account);
        (destination_account, shared_secret)
    }

    /// Rederive the `shared_secret` from a `destination_account`.
    ///
    /// Although it is not strictly necessary, this uses the same logic as the Javascript
    /// STREAM server. Because this STREAM server is intended to be used as part of a node with
    /// forwarding capabilities, rather than as a standalone receiver, it will try forwarding
    /// any packets that it is unable to decrypt. An alternative algorithm for rederiving
    /// the shared secret could include an auth tag to definitively check whether the packet
    /// is meant for this receiver before attempting to decrypt the packet. That will be more
    /// important if/when we want to use STREAM for sending larger amounts of data and want
    /// to avoid copying the STREAM data packet before decrypting it.
    ///
    /// This method returns a Result in case we want to change the internal
    /// logic in the future.
    pub fn rederive_secret(&self, destination_account: &Address) -> Result<[u8; 32], ()> {
        let local_part = destination_account.segments().rev().next().unwrap();
        // Note this computes the HMAC with the token _encoded as UTF8_,
        // rather than decoding the base64 first.
        let shared_secret = hmac_sha256(&self.secret_generator[..], &local_part.as_bytes()[..]);
        Ok(shared_secret)
    }
}

/// Notification that STREAM fulfilled a packet and received a single Interledger payment, used by Pubsub API consumers
#[derive(Debug, Deserialize, Serialize)]
pub struct PaymentNotification {
    /// The username of the account that received the Interledger payment
    pub to_username: Username,
    /// The username of the account that routed the Interledger payment to this node
    pub from_username: Username,
    /// The ILP Address of the receiver of the payment notification
    pub destination: Address,
    /// The amount received
    pub amount: u64,
    /// The time this payment notification was fired in RFC3339 format
    pub timestamp: String,
}

/// A trait representing the Publish side of a pub/sub store
pub trait StreamNotificationsStore {
    type Account: Account;

    /// *Synchronously* saves the sending side of the provided account id's websocket channel to the store's memory
    fn add_payment_notification_subscription(
        &self,
        account_id: Uuid,
        sender: UnboundedSender<PaymentNotification>,
    );

    /// Instructs the store to publish the provided payment notification object
    /// via its Pubsub interface
    fn publish_payment_notification(&self, _payment: PaymentNotification);
}

/// An OutgoingService that fulfills incoming STREAM packets.
///
/// Note this does **not** maintain STREAM state, but instead fulfills
/// all incoming packets to collect the money.
///
/// This does not currently support handling data sent via STREAM.
#[derive(Clone)]
pub struct StreamReceiverService<S, O: OutgoingService<A>, A: Account> {
    connection_generator: ConnectionGenerator,
    next: O,
    account_type: PhantomData<A>,
    store: S,
}

impl<S, O, A> StreamReceiverService<S, O, A>
where
    S: StreamNotificationsStore<Account = A>,
    O: OutgoingService<A>,
    A: Account,
{
    pub fn new(server_secret: Bytes, store: S, next: O) -> Self {
        let connection_generator = ConnectionGenerator::new(server_secret);
        StreamReceiverService {
            connection_generator,
            next,
            account_type: PhantomData,
            store,
        }
    }
}

#[async_trait]
impl<S, O, A> OutgoingService<A> for StreamReceiverService<S, O, A>
where
    S: StreamNotificationsStore + Send + Sync + 'static + Clone,
    O: OutgoingService<A> + Send + Sync + Clone,
    A: Account + Send + Sync + Clone,
{
    /// Try fulfilling the request if it is for this STREAM server or pass it to the next
    /// outgoing handler if not.
    async fn send_request(&mut self, request: OutgoingRequest<A>) -> IlpResult {
        let to_username = request.to.username().clone();
        let from_username = request.from.username().clone();
        let amount = request.prepare.amount();
        let store = self.store.clone();

        let destination = request.prepare.destination();
        let to_address = request.to.ilp_address();
        let dest: &[u8] = destination.as_ref();

        // The case where the request is bound for this server
        if dest.starts_with(to_address.as_ref()) {
            if let Ok(shared_secret) = self.connection_generator.rederive_secret(&destination) {
                let response = receive_money(
                    &shared_secret,
                    &to_address,
                    request.to.asset_code(),
                    request.to.asset_scale(),
                    &request.prepare,
                );
                match response {
                    Ok(ref _fulfill) => store.publish_payment_notification(PaymentNotification {
                        to_username,
                        from_username,
                        amount,
                        destination: destination.clone(),
                        timestamp: DateTime::<Utc>::from(SystemTime::now()).to_rfc3339(),
                    }),
                    Err(ref reject) => {
                        if reject.code() == ErrorCode::F06_UNEXPECTED_PAYMENT {
                            // Assume the packet isn't for us if the decryption step fails.
                            // Note this means that if the packet data is modified in any way,
                            // the sender will likely see an error like F02: Unavailable (this is
                            // a bit confusing but the packet data should not be modified at all
                            // under normal circumstances).
                            return self.next.send_request(request).await;
                        }
                    }
                };
                return response;
            }
        }
        self.next.send_request(request).await
    }
}

// TODO send asset code and scale back to sender also
fn receive_money(
    shared_secret: &[u8; 32],
    // Our node's ILP Address ( we are the receiver, so we should return that
    // plus any other relevant information in our prepare packet's frames)
    ilp_address: &Address,
    asset_code: &str,
    asset_scale: u8,
    prepare: &Prepare,
) -> Result<Fulfill, Reject> {
    // Generate fulfillment
    let fulfillment = generate_fulfillment(&shared_secret[..], prepare.data());
    let condition = hash_sha256(&fulfillment);
    let is_fulfillable = condition == prepare.execution_condition();

    // Parse STREAM packet
    // TODO avoid copying data
    let prepare_amount = prepare.amount();

    // Note that we are copying the Prepare packet data. This is a bad idea
    // in cases where STREAM is used to send a significant amount of data.
    // This implementation doesn't currently support handling the STREAM data
    // so copying the bytes of the other STREAM frames shouldn't be a big
    // performance hit in practice.
    // The data is copied so that we can take the Prepare packet by
    // reference in the case that the decryption fails and we want to pass
    // the request on to the next service.
    let copied_data = BytesMut::from(prepare.data());

    let stream_packet = StreamPacket::from_encrypted(shared_secret, copied_data).map_err(|_| {
        debug!("Unable to parse data, rejecting Prepare packet");
        RejectBuilder {
            code: ErrorCode::F06_UNEXPECTED_PAYMENT,
            message: b"Could not decrypt data",
            triggered_by: Some(ilp_address),
            data: &[],
        }
        .build()
    })?;

    let mut response_frames: Vec<Frame> = Vec::new();

    // Handle STREAM frames
    // TODO reject if they send data?
    for frame in stream_packet.frames() {
        // Tell the sender the stream can handle lots of money
        if let Frame::StreamMoney(ref frame) = frame {
            response_frames.push(Frame::StreamMaxMoney(StreamMaxMoneyFrame {
                stream_id: frame.stream_id,
                // TODO will returning zero here cause problems?
                total_received: 0,
                receive_max: u64::max_value(),
            }));
        }

        // If we receive a ConnectionNewAddress frame, then send them our asset
        // code & scale. The client is suppoesd to only send the
        // ConnectionNewAddress frame once, so we expect that we will only have
        // to respond with the ConnectionAssetDetails frame only one time.
        if let Frame::ConnectionNewAddress(_) = frame {
            response_frames.push(Frame::ConnectionAssetDetails(ConnectionAssetDetailsFrame {
                source_asset_code: asset_code,
                source_asset_scale: asset_scale,
            }));
        }
    }

    // Return Fulfill or Reject Packet
    if is_fulfillable && prepare_amount >= stream_packet.prepare_amount() {
        let response_packet = StreamPacketBuilder {
            sequence: stream_packet.sequence(),
            ilp_packet_type: IlpPacketType::Fulfill,
            prepare_amount,
            frames: &response_frames,
        }
        .build();
        debug!(
            "Fulfilling prepare for amount {} with fulfillment: {} and encrypted stream packet: {:?}",
            prepare_amount,
            hex::encode(&fulfillment[..]),
            response_packet
        );
        let encrypted_response = response_packet.into_encrypted(shared_secret);
        let fulfill = FulfillBuilder {
            fulfillment: &fulfillment,
            data: &encrypted_response[..],
        }
        .build();
        Ok(fulfill)
    } else {
        let response_packet = StreamPacketBuilder {
            sequence: stream_packet.sequence(),
            ilp_packet_type: IlpPacketType::Reject,
            prepare_amount,
            frames: &response_frames,
        }
        .build();
        if !is_fulfillable {
            debug!("Packet is unfulfillable");
        } else if prepare_amount < stream_packet.prepare_amount() {
            debug!(
                "Received only: {} when we should have received at least: {}",
                prepare_amount,
                stream_packet.prepare_amount()
            );
        }
        debug!(
            "Rejecting Prepare and including encrypted stream packet {:?}",
            response_packet
        );
        let encrypted_response = response_packet.into_encrypted(shared_secret);
        let reject = RejectBuilder {
            code: ErrorCode::F99_APPLICATION_ERROR,
            message: &[],
            triggered_by: Some(&ilp_address),
            data: &encrypted_response[..],
        }
        .build();
        Err(reject)
    }
}

#[cfg(test)]
mod connection_generator {
    use super::*;
    use std::str::FromStr;

    #[test]
    fn generates_valid_ilp_address() {
        let server_secret = [9; 32];
        let receiver_address = Address::from_str("example.receiver").unwrap();
        let connection_generator = ConnectionGenerator::new(Bytes::from(&server_secret[..]));
        let (destination_account, shared_secret) =
            connection_generator.generate_address_and_secret(&receiver_address);

        assert!(destination_account
            .to_bytes()
            .starts_with(receiver_address.as_ref()));

        assert_eq!(
            connection_generator
                .rederive_secret(&destination_account)
                .unwrap(),
            shared_secret
        );
    }
}

#[cfg(test)]
fn test_stream_packet() -> StreamPacket {
    StreamPacketBuilder {
        ilp_packet_type: IlpPacketType::Prepare,
        prepare_amount: 0,
        sequence: 1,
        frames: &[Frame::StreamMoney(StreamMoneyFrame {
            stream_id: 1,
            shares: 1,
        })],
    }
    .build()
}

#[cfg(test)]
mod receiving_money {
    use super::*;
    use interledger_packet::PrepareBuilder;
    use std::convert::TryFrom;

    use std::str::FromStr;
    use std::time::UNIX_EPOCH;
    #[test]
    fn fulfills_valid_packet() {
        let ilp_address = Address::from_str("example.destination").unwrap();
        let server_secret = Bytes::from(&[1; 32][..]);
        let connection_generator = ConnectionGenerator::new(server_secret);
        let (destination_account, shared_secret) =
            connection_generator.generate_address_and_secret(&ilp_address);
        let stream_packet = test_stream_packet();
        let data = stream_packet.into_encrypted(&shared_secret[..]);
        let execution_condition = generate_condition(&shared_secret[..], &data);

        let dest = Address::try_from(destination_account).unwrap();
        let prepare = PrepareBuilder {
            destination: dest,
            amount: 100,
            expires_at: UNIX_EPOCH,
            data: &data[..],
            execution_condition: &execution_condition,
        }
        .build();

        let shared_secret = connection_generator
            .rederive_secret(&prepare.destination())
            .unwrap();
        let result = receive_money(&shared_secret, &ilp_address, "ABC", 9, &prepare);
        assert!(result.is_ok());
    }

    #[test]
    fn fulfills_valid_packet_without_connection_tag() {
        let ilp_address = Address::from_str("example.destination").unwrap();
        let server_secret = Bytes::from(&[1; 32][..]);
        let connection_generator = ConnectionGenerator::new(server_secret);
        let (destination_account, shared_secret) =
            connection_generator.generate_address_and_secret(&ilp_address);
        let stream_packet = test_stream_packet();
        let data = stream_packet.into_encrypted(&shared_secret[..]);
        let execution_condition = generate_condition(&shared_secret[..], &data);

        let dest = Address::try_from(destination_account).unwrap();
        let prepare = PrepareBuilder {
            destination: dest,
            amount: 100,
            expires_at: UNIX_EPOCH,
            data: &data[..],
            execution_condition: &execution_condition,
        }
        .build();

        let shared_secret = connection_generator
            .rederive_secret(&prepare.destination())
            .unwrap();
        let result = receive_money(&shared_secret, &ilp_address, "ABC", 9, &prepare);
        assert!(result.is_ok());
    }

    #[test]
    fn rejects_modified_data() {
        let ilp_address = Address::from_str("example.destination").unwrap();
        let server_secret = Bytes::from(&[1; 32][..]);
        let connection_generator = ConnectionGenerator::new(server_secret);
        let (destination_account, shared_secret) =
            connection_generator.generate_address_and_secret(&ilp_address);
        let stream_packet = test_stream_packet();
        let mut data = stream_packet.into_encrypted(&shared_secret[..]);
        data.extend_from_slice(b"x");
        let execution_condition = generate_condition(&shared_secret[..], &data);

        let dest = Address::try_from(destination_account).unwrap();
        let prepare = PrepareBuilder {
            destination: dest,
            amount: 100,
            expires_at: UNIX_EPOCH,
            data: &data[..],
            execution_condition: &execution_condition,
        }
        .build();

        let shared_secret = connection_generator
            .rederive_secret(&prepare.destination())
            .unwrap();
        let result = receive_money(&shared_secret, &ilp_address, "ABC", 9, &prepare);
        assert!(result.is_err());
    }

    #[test]
    fn rejects_too_little_money() {
        let ilp_address = Address::from_str("example.destination").unwrap();
        let server_secret = Bytes::from(&[1; 32][..]);
        let connection_generator = ConnectionGenerator::new(server_secret);
        let (destination_account, shared_secret) =
            connection_generator.generate_address_and_secret(&ilp_address);

        let stream_packet = StreamPacketBuilder {
            ilp_packet_type: IlpPacketType::Prepare,
            prepare_amount: 101,
            sequence: 1,
            frames: &[Frame::StreamMoney(StreamMoneyFrame {
                stream_id: 1,
                shares: 1,
            })],
        }
        .build();

        let data = stream_packet.into_encrypted(&shared_secret[..]);
        let execution_condition = generate_condition(&shared_secret[..], &data);

        let dest = Address::try_from(destination_account).unwrap();
        let prepare = PrepareBuilder {
            destination: dest,
            amount: 100,
            expires_at: UNIX_EPOCH,
            data: &data[..],
            execution_condition: &execution_condition,
        }
        .build();

        let shared_secret = connection_generator
            .rederive_secret(&prepare.destination())
            .unwrap();
        let result = receive_money(&shared_secret, &ilp_address, "ABC", 9, &prepare);
        assert!(result.is_err());
    }

    #[test]
    fn fulfills_packets_sent_to_javascript_receiver() {
        // This was created by the JS ilp-protocol-stream library
        let ilp_address = Address::from_str("test.peerB").unwrap();
        let prepare = Prepare::try_from(bytes::BytesMut::from(&hex::decode("0c819900000000000001f43230313931303238323134313533383338f31a96346c613011947f39a0f1f4e573c2fc3e7e53797672b01d2898e90c9a0723746573742e70656572422e4e6a584430754a504275477a353653426d4933755836682d3b6cc484c0d4e9282275d4b37c6ae18f35b497ddbfcbce6d9305b9451b4395c3158aa75e05bf27582a237109ec6ca0129d840da7abd96826c8147d0d").unwrap()[..])).unwrap();
        let condition = prepare.execution_condition().to_vec();
        let server_secret = Bytes::from(vec![0u8; 32]);
        let connection_generator = ConnectionGenerator::new(server_secret);
        let shared_secret = connection_generator
            .rederive_secret(&prepare.destination())
            .expect("Receiver should be able to rederive the shared secret");
        assert_eq!(
            &shared_secret[..],
            hex::decode("b7d09d2e16e6f83c55b60e42fcd7c2b8ed49624a1df73c59b383dbe2e8690309")
                .unwrap()
                .as_ref() as &[u8],
            "did not regenerate the same shared secret",
        );
        let fulfill = receive_money(&shared_secret, &ilp_address, "ABC", 9, &prepare)
            .expect("Receiver should be able to generate the fulfillment");
        assert_eq!(
            &hash_sha256(fulfill.fulfillment())[..],
            &condition[..],
            "fulfillment generated does not hash to the expected condition"
        );
    }
}

#[cfg(test)]
mod stream_receiver_service {
    use super::*;
    use crate::test_helpers::*;
    use interledger_packet::PrepareBuilder;
    use interledger_service::outgoing_service_fn;

    use std::convert::TryFrom;
    use std::str::FromStr;
    use std::time::UNIX_EPOCH;

    #[tokio::test]
    async fn fulfills_correct_packets() {
        let ilp_address = Address::from_str("example.destination").unwrap();
        let server_secret = Bytes::from(&[1; 32][..]);
        let connection_generator = ConnectionGenerator::new(server_secret.clone());
        let (destination_account, shared_secret) =
            connection_generator.generate_address_and_secret(&ilp_address);
        let stream_packet = test_stream_packet();
        let data = stream_packet.into_encrypted(&shared_secret[..]);
        let execution_condition = generate_condition(&shared_secret[..], &data);

        let dest = Address::try_from(destination_account).unwrap();
        let prepare = PrepareBuilder {
            destination: dest,
            amount: 100,
            expires_at: UNIX_EPOCH,
            data: &data[..],
            execution_condition: &execution_condition,
        }
        .build();

        let mut service = StreamReceiverService::new(
            server_secret.clone(),
            DummyStore,
            outgoing_service_fn(|_: OutgoingRequest<TestAccount>| -> IlpResult {
                panic!("shouldn't get here")
            }),
        );

        let result = service
            .send_request(OutgoingRequest {
                from: TestAccount {
                    id: Uuid::new_v4(),
                    ilp_address: Address::from_str("example.sender").unwrap(),
                    asset_code: "XYZ".to_string(),
                    asset_scale: 9,
                    max_packet_amount: None,
                },
                to: TestAccount {
                    id: Uuid::new_v4(),
                    ilp_address: ilp_address.clone(),
                    asset_code: "XYZ".to_string(),
                    asset_scale: 9,
                    max_packet_amount: None,
                },
                original_amount: prepare.amount(),
                prepare,
            })
            .await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn rejects_invalid_packets() {
        let ilp_address = Address::from_str("example.destination").unwrap();
        let server_secret = Bytes::from(&[1; 32][..]);
        let connection_generator = ConnectionGenerator::new(server_secret.clone());
        let (destination_account, shared_secret) =
            connection_generator.generate_address_and_secret(&ilp_address);
        let stream_packet = test_stream_packet();
        let mut data = stream_packet.into_encrypted(&shared_secret[..]);
        let execution_condition = generate_condition(&shared_secret[..], &data);

        data.extend_from_slice(b"extra");
        let dest = Address::try_from(destination_account).unwrap();

        let prepare = PrepareBuilder {
            destination: dest,
            amount: 100,
            expires_at: UNIX_EPOCH,
            data: &data[..],
            execution_condition: &execution_condition,
        }
        .build();

        let mut service = StreamReceiverService::new(
            server_secret.clone(),
            DummyStore,
            outgoing_service_fn(
                |_: OutgoingRequest<TestAccount>| -> Result<Fulfill, Reject> {
                    Err(RejectBuilder {
                        code: ErrorCode::F02_UNREACHABLE,
                        message: &[],
                        data: &[],
                        triggered_by: None,
                    }
                    .build())
                },
            ),
        );

        let result = service
            .send_request(OutgoingRequest {
                from: TestAccount {
                    id: Uuid::new_v4(),
                    ilp_address: Address::from_str("example.sender").unwrap(),
                    asset_code: "XYZ".to_string(),
                    asset_scale: 9,
                    max_packet_amount: None,
                },
                to: TestAccount {
                    id: Uuid::new_v4(),
                    ilp_address: ilp_address.clone(),
                    asset_code: "XYZ".to_string(),
                    asset_scale: 9,
                    max_packet_amount: None,
                },
                original_amount: prepare.amount(),
                prepare,
            })
            .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn passes_on_packets_not_for_it() {
        let ilp_address = Address::from_str("example.destination").unwrap();
        let server_secret = Bytes::from(&[1; 32][..]);
        let connection_generator = ConnectionGenerator::new(server_secret.clone());
        let (destination_account, shared_secret) =
            connection_generator.generate_address_and_secret(&ilp_address);
        let stream_packet = test_stream_packet();
        let data = stream_packet.into_encrypted(&shared_secret[..]);
        let execution_condition = generate_condition(&shared_secret[..], &data);

        let dest = Address::try_from(destination_account).unwrap();
        let dest = dest.with_suffix(b"extra").unwrap();

        let prepare = PrepareBuilder {
            destination: dest,
            amount: 100,
            expires_at: UNIX_EPOCH,
            data: &data[..],
            execution_condition: &execution_condition,
        }
        .build();

        let mut service = StreamReceiverService::new(
            server_secret.clone(),
            DummyStore,
            outgoing_service_fn(|_| {
                Err(RejectBuilder {
                    code: ErrorCode::F02_UNREACHABLE,
                    message: &[],
                    data: &[],
                    triggered_by: Address::from_str("example.other-receiver").ok().as_ref(),
                }
                .build())
            }),
        );

        let result = service
            .send_request(OutgoingRequest {
                from: TestAccount {
                    id: Uuid::new_v4(),
                    ilp_address: Address::from_str("example.sender").unwrap(),
                    asset_code: "XYZ".to_string(),
                    asset_scale: 9,
                    max_packet_amount: None,
                },
                original_amount: prepare.amount(),
                to: TestAccount {
                    id: Uuid::new_v4(),
                    ilp_address: ilp_address.clone(),
                    asset_code: "XYZ".to_string(),
                    asset_scale: 9,
                    max_packet_amount: None,
                },
                prepare,
            })
            .await;
        assert!(result.is_err());
        assert_eq!(
            result.unwrap_err().triggered_by().unwrap(),
            Address::from_str("example.other-receiver").unwrap(),
        );
    }
}
