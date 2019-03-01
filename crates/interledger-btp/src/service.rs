use super::packet::*;
use bytes::BytesMut;
use futures::{
    future::{err, ok, result, Either},
    sync::mpsc::{unbounded, UnboundedSender},
    sync::oneshot,
    Future, Sink, Stream,
};
use hashbrown::HashMap;
use interledger_packet::{ErrorCode, Fulfill, Packet, Reject, RejectBuilder};
use interledger_service::*;
use parking_lot::{Mutex, RwLock};
use rand::random;
use std::io::{Error as IoError, ErrorKind};
use std::iter::{FromIterator, IntoIterator};
use std::sync::Arc;
use tokio;
use tokio_tcp::TcpStream;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};
use tungstenite::{error::Error as WebSocketError, Message};

type WsStream = WebSocketStream<MaybeTlsStream<TcpStream>>;

type IlpResultChannel = oneshot::Sender<Result<Fulfill, Reject>>;

#[derive(Clone)]
pub struct BtpService<S, T, A: Account> {
    connections: Arc<RwLock<HashMap<A::AccountId, UnboundedSender<Message>>>>,
    pending_requests: Arc<Mutex<HashMap<u32, IlpResultChannel>>>,
    incoming_handler: S,
    next_outgoing: T,
}

impl<S, T, A> BtpService<S, T, A>
where
    S: IncomingService<A> + Clone + Send + 'static,
    T: OutgoingService<A> + Clone,
    A: Account + 'static,
{
    pub(crate) fn new(incoming_handler: S, next_outgoing: T) -> Self {
        BtpService {
            connections: Arc::new(RwLock::new(HashMap::new())),
            pending_requests: Arc::new(Mutex::new(HashMap::new())),
            incoming_handler,
            next_outgoing,
        }
    }

    pub(crate) fn add_connection(&self, account: A, connection: WsStream) {
        let account_id = account.id();

        // Set up a channel to forward outgoing packets to the WebSocket connection
        let (tx, rx) = unbounded();
        let (sink, stream) = connection.split();
        let forward_to_connection = sink
            .send_all(
                rx.map_err(|_err| {
                    WebSocketError::from(IoError::from(ErrorKind::ConnectionAborted))
                }),
            )
            .map(|_| ())
            .map_err(|_| ());
        tokio::spawn(forward_to_connection);

        // Set up a listener to handle incoming packets from the WebSocket connection
        // TODO do we need all this cloning?
        let tx_clone = tx.clone();
        let mut incoming_handler = self.incoming_handler.clone();
        let pending_requests = self.pending_requests.clone();
        let handle_incoming = stream.map_err(|_err| ()).for_each(move |message| {
          let tx = tx_clone.clone();

          // Handle the packets based on whether they are an incoming request or a response to something we sent
          match parse_ilp_packet(message) {
            Ok((request_id, Packet::Prepare(prepare))) => Either::A(
              incoming_handler
                .handle_request(IncomingRequest {
                  from: account.clone(),
                  prepare,
                })
                .then(move |result| {
                  let packet: Packet = match result {
                    Ok(fulfill) => Packet::Fulfill(fulfill),
                    Err(reject) => Packet::Reject(reject),
                  };
                  let message = ilp_packet_to_ws_message(request_id, packet);
                  tx.unbounded_send(message).map_err(|err| error!("Error sending ILP response back through BTP connection: {:?}", err))
                }),
            ),
            Ok((request_id, Packet::Fulfill(fulfill))) => {
              if let Some(channel) = (*pending_requests.lock()).remove(&request_id) {
                channel.send(Ok(fulfill)).unwrap_or_else(|fulfill| error!("Error forwarding Fulfill packet back to the Future that sent the Prepare: {:?}", fulfill));
              } else {
                warn!("Got Fulfill packet that does not match an outgoing Prepare we sent: {:?}", fulfill);
              }
              Either::B(ok(()))
            }
            Ok((request_id, Packet::Reject(reject))) => {
              if let Some(channel) = (*pending_requests.lock()).remove(&request_id) {
                channel.send(Err(reject)).unwrap_or_else(|reject| error!("Error forwarding Reject packet back to the Future that sent the Prepare: {:?}", reject));
              } else {
                warn!("Got Reject packet that does not match an outgoing Prepare we sent: {:?}", reject);
              }
              Either::B(ok(()))
            },
            Err(_) => {
              debug!("Unable to parse ILP packet from BTP packet");
              // TODO Send error back
              Either::B(ok(()))
            }
          }
        }).then(|result| {
            debug!("WebSocket stream ended");
            result
        });
        tokio::spawn(handle_incoming);

        // Save the sender side of the channel so we have a way to forward outgoing requests to the WebSocket
        self.connections.write().insert(account_id, tx);
    }
}

impl<S, T, A> OutgoingService<A> for BtpService<S, T, A>
where
    T: OutgoingService<A> + Clone + Send + 'static,
    A: Account,
{
    type Future = BoxedIlpFuture;

    fn send_request(&mut self, request: OutgoingRequest<A>) -> Self::Future {
        if let Some(connection) = (*self.connections.read()).get(&request.to.id()) {
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
                request.to.id()
            );
            Box::new(self.next_outgoing.send_request(request))
        }
    }
}

fn parse_ilp_packet(message: Message) -> Result<(u32, Packet), ()> {
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

fn ilp_packet_to_ws_message(request_id: u32, packet: Packet) -> Message {
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
