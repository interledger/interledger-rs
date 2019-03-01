use super::crypto::*;
use super::packet::*;
use base64;
use bytes::{BufMut, Bytes, BytesMut};
use futures::future::result;
use hex;
use interledger_ildcp::IldcpResponse;
use interledger_packet::{
    ErrorCode, Fulfill, FulfillBuilder, PacketType as IlpPacketType, Prepare, Reject, RejectBuilder,
};
use interledger_service::*;
use parking_lot::RwLock;
use std::marker::PhantomData;
use std::str;
use std::sync::Arc;

lazy_static! {
    static ref TAG_ENCRYPTION_KEY_STRING: &'static [u8] = b"ilp_stream_tag_encryption_aes";
}

#[derive(Clone)]
pub struct ConnectionGenerator {
    source_account: Bytes,
    server_secret: Bytes,
}

impl ConnectionGenerator {
    pub fn new(source_account: &[u8], server_secret: &[u8]) -> Self {
        ConnectionGenerator {
            source_account: Bytes::from(source_account),
            server_secret: Bytes::from(server_secret),
        }
    }

    pub fn generate_address_and_secret(&self, connection_tag: &[u8]) -> (Bytes, [u8; 32]) {
        let token_bytes = generate_token();
        let token = base64::encode_config(&token_bytes, base64::URL_SAFE_NO_PAD);

        // TODO include the connection_tag in the shared secret
        // so removing it would cause the packets to be rejected
        let shared_secret =
            generate_shared_secret_from_token(&self.server_secret, &token.as_bytes());
        let destination_account = if connection_tag.is_empty() {
            let mut account = BytesMut::with_capacity(self.source_account.len() + 1 + token.len());
            account.put(&self.source_account[..]);
            account.put_u8(b'.');
            account.put(token);
            account.freeze()
        } else {
            let encrypted_tag = encrypt_tag(&self.server_secret, connection_tag);
            // TODO don't use the ~ so it's harder to identify which part is which from the outside
            let mut account = BytesMut::with_capacity(
                self.source_account.len() + 2 + token.len() + encrypted_tag.len(),
            );
            account.put(&self.source_account[..]);
            account.put_u8(b'.');
            account.put(token);
            account.put_u8(b'~');
            account.put(encrypted_tag);
            account.freeze()
        };
        (destination_account, shared_secret)
    }
}

#[derive(Clone)]
pub struct StreamReceiverService<S: IncomingService<A>, A: Account> {
    server_secret: Bytes,
    ildcp_response: Arc<RwLock<Option<IldcpResponse>>>,
    next: S,
    account_type: PhantomData<A>,
}

impl<S, A> StreamReceiverService<S, A>
where
    S: IncomingService<A>,
    A: Account,
{
    pub fn new(server_secret: &[u8; 32], ildcp_response: IldcpResponse, next: S) -> Self {
        StreamReceiverService {
            server_secret: Bytes::from(&server_secret[..]),
            ildcp_response: Arc::new(RwLock::new(Some(ildcp_response))),
            next,
            account_type: PhantomData,
        }
    }

    /// For creating a receiver service before the ILDCP details have been received
    pub fn without_ildcp(server_secret: &[u8; 32], next: S) -> Self {
        StreamReceiverService {
            server_secret: Bytes::from(&server_secret[..]),
            ildcp_response: Arc::new(RwLock::new(None)),
            next,
            account_type: PhantomData,
        }
    }

    pub fn set_ildcp(&self, ildcp_response: IldcpResponse) {
        self.ildcp_response.write().replace(ildcp_response);
    }
}

// TODO should this be an OutgoingService instead so the balance logic is applied before this is called?
impl<S, A> IncomingService<A> for StreamReceiverService<S, A>
where
    S: IncomingService<A>,
    A: Account,
{
    type Future = BoxedIlpFuture;

    fn handle_request(&mut self, request: IncomingRequest<A>) -> Self::Future {
        if let Some(ref ildcp_response) = *self.ildcp_response.read() {
            if request
                .prepare
                .destination()
                .starts_with(ildcp_response.client_address())
            {
                return Box::new(result(receive_money(
                    &self.server_secret[..],
                    ildcp_response,
                    request.prepare,
                )));
            }
        } else {
            warn!("Got incoming Prepare packet before the StreamReceiverService was ready (before it had the ILDCP info)");
        }
        // TODO if it's not ready yet should we respond with an error instead?
        Box::new(self.next.handle_request(request))
    }
}

fn encrypt_tag(server_secret: &[u8], tag: &[u8]) -> String {
    let key = hmac_sha256(server_secret, &TAG_ENCRYPTION_KEY_STRING);
    let encrypted = encrypt(&key[..], BytesMut::from(tag));
    base64::encode_config(&encrypted[..], base64::URL_SAFE_NO_PAD)
}

fn decrypt_tag(server_secret: &[u8], encrypted: &[u8]) -> String {
    let key = hmac_sha256(server_secret, &TAG_ENCRYPTION_KEY_STRING);
    let decoded =
        base64::decode_config(encrypted, base64::URL_SAFE_NO_PAD).unwrap_or_else(|_| Vec::new());
    let decrypted =
        decrypt(&key[..], BytesMut::from(&decoded[..])).unwrap_or_else(|_| BytesMut::new());
    let decrypted_vec = decrypted.freeze().to_vec();
    String::from_utf8(decrypted_vec).unwrap_or_else(|_| String::new())
}

fn derive_shared_secret<'a>(
    server_secret: &[u8],
    local_address: &'a [u8],
    prepare: &Prepare,
) -> Result<(String, [u8; 32]), Reject> {
    let local_address_parts: Vec<&[u8]> = local_address.split(|c| *c == b'.').collect();
    if local_address_parts.is_empty() {
        warn!(
            "Got Prepare with no Connection ID: {}",
            str::from_utf8(prepare.destination()).unwrap_or("(cannot convert address to ASCII)")
        );
        return Err(RejectBuilder {
            code: ErrorCode::F02_UNREACHABLE,
            message: &[],
            triggered_by: &[],
            data: &[],
        }
        .build());
    }
    let connection_id = local_address_parts[0];

    let split: Vec<&[u8]> = connection_id.splitn(2, |c| *c == b'~').collect();
    let token = split[0];
    let shared_secret = generate_shared_secret_from_token(&server_secret, token);
    if split.len() == 1 {
        let connection_id = String::from_utf8(connection_id.to_vec()).unwrap_or_default();
        Ok((connection_id, shared_secret))
    } else {
        let encrypted_tag = split[1];
        let decrypted = decrypt_tag(&server_secret[..], encrypted_tag);
        // TODO don't mash these two together, just return them separately
        let connection_id = format!(
            "{}~{}",
            str::from_utf8(token).unwrap_or_default(),
            decrypted
        );
        Ok((connection_id, shared_secret))
    }
}

fn receive_money(
    server_secret: &[u8],
    ildcp_response: &IldcpResponse,
    prepare: Prepare,
) -> Result<Fulfill, Reject> {
    // Generate shared secret
    if prepare.destination().len() < ildcp_response.client_address().len() + 1 {
        debug!("Got Prepare packet with no token attached to the destination address");
        return Err(RejectBuilder {
            code: ErrorCode::F02_UNREACHABLE,
            message: &[],
            triggered_by: ildcp_response.client_address(),
            data: &[],
        }
        .build());
    }
    let local_address = prepare
        .destination()
        .split_at(ildcp_response.client_address().len() + 1)
        .1;
    let (_conn_id, shared_secret) = derive_shared_secret(server_secret, &local_address, &prepare)?;

    // Generate fulfillment
    let fulfillment = generate_fulfillment(&shared_secret[..], prepare.data());
    let condition = fulfillment_to_condition(&fulfillment);
    let is_fulfillable = condition == prepare.execution_condition();

    // Parse STREAM packet
    // TODO avoid copying data
    let prepare_amount = prepare.amount();
    let stream_packet =
        StreamPacket::from_encrypted(&shared_secret, prepare.into_data()).map_err(|_| {
            debug!("Unable to parse data, rejecting Prepare packet");
            RejectBuilder {
                code: ErrorCode::F02_UNREACHABLE,
                message: &[],
                triggered_by: ildcp_response.client_address(),
                data: &[],
            }
            .build()
        })?;

    let mut response_frames: Vec<Frame> = Vec::new();

    // Handle STREAM frames
    // TODO reject if they send data?
    for frame in stream_packet.frames() {
        // Tell the sender the stream can handle lots of money
        if let Frame::StreamMoney(frame) = frame {
            response_frames.push(Frame::StreamMaxMoney(StreamMaxMoneyFrame {
                stream_id: frame.stream_id,
                // TODO will returning zero here cause problems?
                total_received: 0,
                receive_max: u64::max_value(),
            }));
        }
    }

    // Return Fulfill or Reject Packet
    if is_fulfillable {
        let response_packet = StreamPacketBuilder {
            sequence: stream_packet.sequence(),
            ilp_packet_type: IlpPacketType::Fulfill,
            prepare_amount,
            frames: &response_frames,
        }
        .build();
        debug!(
            "Fulfilling prepare with fulfillment: {} and encrypted stream packet: {:?}",
            hex::encode(&fulfillment[..]),
            response_packet
        );
        let encrypted_response = response_packet.into_encrypted(&shared_secret);
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
        debug!(
            "Rejecting Prepare and including encrypted stream packet {:?}",
            response_packet
        );
        let encrypted_response = response_packet.into_encrypted(&shared_secret);
        let reject = RejectBuilder {
            code: ErrorCode::F99_APPLICATION_ERROR,
            message: &[],
            triggered_by: ildcp_response.client_address(),
            data: &encrypted_response[..],
        }
        .build();
        Err(reject)
    }
}
