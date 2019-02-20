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
use std::io::{Error as IoError, ErrorKind};
use std::iter::FromIterator;
use std::sync::Arc;
use tokio;
use tokio_tungstenite::{accept_async, connect_async};
use tungstenite::{error::Error as WebSocketError, Message};

// TODO also add BTP server

type IlpResultChannel = oneshot::Sender<Result<Fulfill, Reject>>;

pub fn connect_client<S, T, W>(
    next: S,
    store: T,
    accounts: Vec<AccountId>,
) -> impl Future<Item = BtpService<S>, Error = ()>
where
    S: IncomingService + Clone + Send + Sync + 'static,
    // TODO do these need to be cloneable?
    T: BtpStore + 'static,
{
    let pending_requests: Arc<Mutex<HashMap<u32, IlpResultChannel>>> =
        Arc::new(Mutex::new(HashMap::new()));
    join_all(accounts.into_iter().map(move |account_id| {
    store
      .get_btp_url(&account_id)
      .and_then(|url| {
        connect_async(url.clone()).map_err(|_err| ())
        .and_then(move |(connection, _)| {
          // Send BTP authentication
          let auth_packet = Message::Binary(BtpPacket::Message(BtpMessage {
              request_id: random(),
              protocol_data: vec![
                  ProtocolData {
                      protocol_name: String::from("auth"),
                      content_type: ContentType::ApplicationOctetStream,
                      data: vec![],
                  },
                  ProtocolData {
                      protocol_name: String::from("auth_username"),
                      content_type: ContentType::TextPlainUtf8,
                      data: String::from(url.username()).into_bytes(),
                  },
                  ProtocolData {
                      protocol_name: String::from("auth_token"),
                      content_type: ContentType::TextPlainUtf8,
                      data: String::from(url.password().unwrap()).into_bytes(),
                  },
              ],
          }).to_bytes());

          connection.send(auth_packet).map_err(move |_| error!("Error sending auth packet on connection: {}", url))
        })
      })
      .and_then(move |connection| Ok((account_id, connection)))
  }))
  .map_err(|_err| ())
  .and_then(|connections| {
    let connections =
      HashMap::from_iter(connections.into_iter().map(|(account_id, connection)| {
        let (tx, rx) = unbounded();
        let (sink, stream) = connection.split();

        let forward_to_connection = sink
          .send_all(
            rx.map_err(|_err| WebSocketError::from(IoError::from(ErrorKind::ConnectionAborted))),
          )
          .map(|_| ())
          .map_err(|_| ());
        tokio::spawn(forward_to_connection);

        // TODO do we need all this cloning?
        let tx_clone = tx.clone();
        let mut next = next.clone();
        let pending_requests = pending_requests.clone();
        let handle_incoming = stream.map_err(|_err| ()).for_each(move |message| {
          let tx = tx_clone.clone();
          match parse_ilp_packet(message) {
            Ok((request_id, Packet::Prepare(prepare))) => Either::A(
              next
                .handle_request(IncomingRequest {
                  from: account_id,
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
                // unexpected fulfill packet
              }
              Either::B(ok(()))
            }
            Ok((request_id, Packet::Reject(reject))) => {
              if let Some(channel) = (*pending_requests.lock()).remove(&request_id) {
                channel.send(Err(reject)).unwrap_or_else(|reject| error!("Error forwarding Reject packet back to the Future that sent the Prepare: {:?}", reject));
              } else {
                // unexpected fulfill packet
              }
              Either::B(ok(()))
            },
            Err(_) => {
              // unexpected BTP packet
              // TODO Send error back
              Either::B(ok(()))
            }
          }
        });
        tokio::spawn(handle_incoming);

        (account_id, tx)
      }));
    Ok(BtpService {
      connections: Arc::new(RwLock::new(connections)),
      pending_requests,
      // store,
      next,
    })
  })
}

#[derive(Clone)]
pub struct BtpService<S> {
    connections: Arc<RwLock<HashMap<AccountId, UnboundedSender<Message>>>>,
    pending_requests: Arc<Mutex<HashMap<u32, IlpResultChannel>>>,
    // store: T,
    next: S,
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

            if connection
                .unbounded_send(Message::from(BytesMut::from(request.prepare).to_vec()))
                .is_ok()
            {
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
            } else {
                let reject = RejectBuilder {
                    code: ErrorCode::T00_INTERNAL_ERROR,
                    message: &[],
                    triggered_by: &[],
                    data: &[],
                }
                .build();
                Box::new(err(reject))
            }
        } else {
            Box::new(self.next.send_request(request))
        }
    }
}

// Passthrough implementation so this can be chained with other services like an HTTP Server
impl<S> IncomingService for BtpService<S>
where
    S: IncomingService,
{
    type Future = BoxedIlpFuture;

    fn handle_request(&mut self, request: IncomingRequest) -> Self::Future {
        Box::new(self.next.handle_request(request))
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
            _ => {
                return Err(());
            }
        };
        if let Ok(packet) = Packet::try_from(BytesMut::from(ilp_data)) {
            Ok((request_id, packet))
        } else {
            Err(())
        }
    } else {
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
