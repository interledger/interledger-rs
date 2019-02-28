use super::packet::*;
use super::BtpStore;
use bytes::BytesMut;
use futures::{
    future::{err, join_all, ok, Either},
    sync::mpsc::{unbounded, UnboundedSender},
    sync::oneshot,
    Future, Sink, Stream,
};
use hashbrown::HashMap;
use interledger_packet::{ErrorCode, Fulfill, Packet, Reject, RejectBuilder};
use interledger_service::*;
use parking_lot::{Mutex, RwLock};
use rand::random;
use std::iter::IntoIterator;
use std::sync::Arc;
use tungstenite::Message;

pub(crate) type IlpResultChannel = oneshot::Sender<Result<Fulfill, Reject>>;

#[derive(Clone)]
pub struct BtpService<S> {
    pub(crate) connections: Arc<RwLock<HashMap<AccountId, UnboundedSender<Message>>>>,
    pub(crate) pending_requests: Arc<Mutex<HashMap<u32, IlpResultChannel>>>,
    // store: T,
    pub(crate) next: S,
}

impl<S> OutgoingService for BtpService<S>
where
    S: OutgoingService + Clone + Send + Sync + 'static,
    // T: BtpStore + Clone + Send + Sync + 'static,
{
    type Future = BoxedIlpFuture;

    fn send_request(&mut self, request: OutgoingRequest) -> Self::Future {
        if let Some(connection) = (*self.connections.read()).get(&request.to) {
            let request_id = random::<u32>();

            match connection.unbounded_send(ilp_packet_to_ws_message(
                request_id,
                Packet::Prepare(request.prepare),
            )) {
                Ok(_) => {
                    let (sender, receiver) = oneshot::channel();
                    (*self.pending_requests.lock()).insert(request_id, sender);
                    Box::new(
                        receiver
                            .map_err(|_| {
                                RejectBuilder {
                                    code: ErrorCode::T00_INTERNAL_ERROR,
                                    message: &[],
                                    triggered_by: &[],
                                    data: &[],
                                }
                                .build()
                            })
                            .and_then(|result| match result {
                                Ok(fulfill) => Ok(fulfill),
                                Err(reject) => Err(reject),
                            }),
                    )
                }
                Err(send_error) => {
                    error!("Error sending websocket message: {:?}", send_error);
                    let reject = RejectBuilder {
                        code: ErrorCode::T00_INTERNAL_ERROR,
                        message: &[],
                        triggered_by: &[],
                        data: &[],
                    }
                    .build();
                    Box::new(err(reject))
                }
            }
        } else {
            debug!(
                "No open connection for account: {}, forwarding request to the next service",
                request.to
            );
            Box::new(self.next.send_request(request))
        }
    }
}

pub(crate) fn parse_ilp_packet(message: Message) -> Result<(u32, Packet), ()> {
    if let Message::Binary(data) = message {
        let (request_id, ilp_data) = match BtpPacket::from_bytes(&data) {
            Ok(BtpPacket::Message(message)) => {
                let ilp_data = message
                    .protocol_data
                    .into_iter()
                    .find(|proto| proto.protocol_name == "ilp")
                    .ok_or(())?
                    .data;
                (message.request_id, ilp_data)
            }
            Ok(BtpPacket::Response(response)) => {
                let ilp_data = response
                    .protocol_data
                    .into_iter()
                    .find(|proto| proto.protocol_name == "ilp")
                    .ok_or(())?
                    .data;
                (response.request_id, ilp_data)
            }
            Ok(BtpPacket::Error(error)) => {
                error!("Got BTP error: {:?}", error);
                return Err(());
            }
            Err(err) => {
                error!("Error parsing BTP packet: {:?}", err);
                return Err(());
            }
        };
        if let Ok(packet) = Packet::try_from(BytesMut::from(ilp_data)) {
            Ok((request_id, packet))
        } else {
            Err(())
        }
    } else {
        error!("Got a non-binary WebSocket message");
        Err(())
    }
}

pub(crate) fn ilp_packet_to_ws_message(request_id: u32, packet: Packet) -> Message {
    match packet {
        Packet::Prepare(prepare) => {
            let data = BytesMut::from(prepare).to_vec();
            let btp_packet = BtpMessage {
                request_id,
                protocol_data: vec![ProtocolData {
                    protocol_name: "ilp".to_string(),
                    content_type: ContentType::ApplicationOctetStream,
                    data,
                }],
            };
            Message::binary(btp_packet.to_bytes())
        }
        Packet::Fulfill(fulfill) => {
            let data = BytesMut::from(fulfill).to_vec();
            let btp_packet = BtpResponse {
                request_id,
                protocol_data: vec![ProtocolData {
                    protocol_name: "ilp".to_string(),
                    content_type: ContentType::ApplicationOctetStream,
                    data,
                }],
            };
            Message::binary(btp_packet.to_bytes())
        }
        Packet::Reject(reject) => {
            let data = BytesMut::from(reject).to_vec();
            let btp_packet = BtpResponse {
                request_id,
                protocol_data: vec![ProtocolData {
                    protocol_name: "ilp".to_string(),
                    content_type: ContentType::ApplicationOctetStream,
                    data,
                }],
            };
            Message::binary(btp_packet.to_bytes())
        }
    }
}
