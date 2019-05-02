use super::packet::*;
use bytes::BytesMut;
use futures::{
    future::err,
    sync::mpsc::{unbounded, UnboundedReceiver, UnboundedSender},
    sync::oneshot,
    Future, Sink, Stream,
};
use hashbrown::HashMap;
use interledger_packet::{ErrorCode, Fulfill, Packet, Prepare, Reject, RejectBuilder};
use interledger_service::*;
use parking_lot::{Mutex, RwLock};
use rand::random;
use std::{
    io::{Error as IoError, ErrorKind},
    iter::IntoIterator,
    marker::PhantomData,
    sync::Arc,
};
use stream_cancel::{Trigger, Valve, Valved};
use tokio_executor::spawn;
use tokio_tcp::TcpStream;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};
use tungstenite::{error::Error as WebSocketError, Message};

type WsStream = WebSocketStream<MaybeTlsStream<TcpStream>>;
type IlpResultChannel = oneshot::Sender<Result<Fulfill, Reject>>;
type IncomingRequestBuffer<A> = UnboundedReceiver<(A, u32, Prepare)>;

/// A container for BTP/WebSocket connections that implements OutgoingService
/// for sending outgoing ILP Prepare packets over one of the connected BTP connections.
#[derive(Clone)]
pub struct BtpOutgoingService<T, A: Account> {
    // TODO support multiple connections per account
    connections: Arc<RwLock<HashMap<A::AccountId, UnboundedSender<Message>>>>,
    pending_outgoing: Arc<Mutex<HashMap<u32, IlpResultChannel>>>,
    pending_incoming: Arc<Mutex<Option<IncomingRequestBuffer<A>>>>,
    incoming_sender: UnboundedSender<(A, u32, Prepare)>,
    next_outgoing: T,
    close_all_connections: Arc<Mutex<Option<Trigger>>>,
    stream_valve: Arc<Valve>,
}

impl<T, A> BtpOutgoingService<T, A>
where
    T: OutgoingService<A> + Clone,
    A: Account + 'static,
{
    pub fn new(next_outgoing: T) -> Self {
        let (incoming_sender, incoming_receiver) = unbounded();
        let (close_all_connections, stream_valve) = Valve::new();
        BtpOutgoingService {
            connections: Arc::new(RwLock::new(HashMap::new())),
            pending_outgoing: Arc::new(Mutex::new(HashMap::new())),
            pending_incoming: Arc::new(Mutex::new(Some(incoming_receiver))),
            incoming_sender,
            next_outgoing,
            close_all_connections: Arc::new(Mutex::new(Some(close_all_connections))),
            stream_valve: Arc::new(stream_valve),
        }
    }

    /// Close all of the open WebSocket connections
    // TODO is there some more automatic way of knowing when we should close the connections?
    // The problem is that the WS client can be a server too, so it's not clear when we are done with it
    pub fn close(&self) {
        debug!("Closing all WebSocket connections");
        self.close_all_connections.lock().take();
    }

    /// Set up a WebSocket connection so that outgoing Prepare packets can be sent to it,
    /// incoming Prepare packets are buffered in a channel (until an IncomingService is added
    /// via the handle_incoming method), and ILP Fulfill and Reject packets will be
    /// sent back to the Future that sent the outgoing request originally.
    pub(crate) fn add_connection(&self, account: A, connection: WsStream) {
        let account_id = account.id();

        // Set up a channel to forward outgoing packets to the WebSocket connection
        let (tx, rx) = unbounded();
        let (sink, stream) = connection.split();
        let (close_connection, stream) = Valved::new(stream);
        let stream = self.stream_valve.wrap(stream);
        let forward_to_connection = sink
            .send_all(
                rx.map_err(|_err| {
                    WebSocketError::from(IoError::from(ErrorKind::ConnectionAborted))
                }),
            )
            .then(move |_| {
                debug!("Finished forwarding to WebSocket stream");
                drop(close_connection);
                Ok(())
            });

        // Set up a listener to handle incoming packets from the WebSocket connection
        // TODO do we need all this cloning?
        let pending_requests = self.pending_outgoing.clone();
        let incoming_sender = self.incoming_sender.clone();
        let handle_incoming = stream.map_err(move |err| error!("Error reading from WebSocket stream for account {}: {:?}", account_id, err)).for_each(move |message| {
          // Handle the packets based on whether they are an incoming request or a response to something we sent
          match parse_ilp_packet(message) {
            Ok((request_id, Packet::Prepare(prepare))) => {
                incoming_sender.clone().unbounded_send((account.clone(), request_id, prepare))
                    .map_err(|err| error!("Unable to buffer incoming request: {:?}", err))
            },
            Ok((request_id, Packet::Fulfill(fulfill))) => {
              trace!("Got fulfill response to request id {}", request_id);
              if let Some(channel) = (*pending_requests.lock()).remove(&request_id) {
                channel.send(Ok(fulfill)).map_err(|fulfill| error!("Error forwarding Fulfill packet back to the Future that sent the Prepare: {:?}", fulfill))
              } else {
                warn!("Got Fulfill packet that does not match an outgoing Prepare we sent: {:?}", fulfill);
                Ok(())
              }
            }
            Ok((request_id, Packet::Reject(reject))) => {
              trace!("Got reject response to request id {}", request_id);
              if let Some(channel) = (*pending_requests.lock()).remove(&request_id) {
                channel.send(Err(reject)).map_err(|reject| error!("Error forwarding Reject packet back to the Future that sent the Prepare: {:?}", reject))
              } else {
                warn!("Got Reject packet that does not match an outgoing Prepare we sent: {:?}", reject);
                Ok(())
              }
            },
            Err(_) => {
              debug!("Unable to parse ILP packet from BTP packet (if this is the first time this appears, the packet was probably the auth response)");
              // TODO Send error back
              Ok(())
            }
          }
        });

        let connections = self.connections.clone();
        let keep_connections_open = self.close_all_connections.clone();
        let handle_connection = handle_incoming
            .select(forward_to_connection)
            .then(move |_| {
                let _ = keep_connections_open;
                let mut connections = connections.write();
                connections.remove(&account_id);
                debug!(
                    "WebSocket connection closed for account {} ({} connections still open)",
                    account_id,
                    connections.len()
                );
                Ok(())
            });
        spawn(handle_connection);

        // Save the sender side of the channel so we have a way to forward outgoing requests to the WebSocket
        self.connections.write().insert(account_id, tx);
    }

    /// Convert this BtpOutgoingService into a bidirectional BtpService by adding a handler for incoming requests.
    /// This will automatically pull all incoming Prepare packets from the channel buffer and call the IncomingService with them.
    pub fn handle_incoming<S>(self, incoming_handler: S) -> BtpService<S, T, A>
    where
        S: IncomingService<A> + Clone + Send + 'static,
    {
        // Any connections that were added to the BtpOutgoingService will just buffer
        // the incoming Prepare packets they get in self.pending_incoming
        // Now that we're adding an incoming handler, this will spawn a task to read
        // all Prepare packets from the buffer, handle them, and send the responses back
        let mut incoming_handler_clone = incoming_handler.clone();
        let connections_clone = self.connections.clone();
        let handle_pending_incoming = self
            .pending_incoming
            .lock()
            .take()
            .expect("handle_incoming can only be called once")
            .for_each(move |(account, request_id, prepare)| {
                let account_id = account.id();
                let connections_clone = connections_clone.clone();
                let request = IncomingRequest {
                    from: account,
                    prepare,
                };
                trace!(
                    "Handling incoming request {} from account {}",
                    request_id,
                    request.from.id()
                );
                incoming_handler_clone
                    .handle_request(request)
                    .then(move |result| {
                        let packet = match result {
                            Ok(fulfill) => Packet::Fulfill(fulfill),
                            Err(reject) => Packet::Reject(reject),
                        };
                        let message = ilp_packet_to_ws_message(request_id, packet);
                        connections_clone
                            .read()
                            .get(&account_id)
                            .expect(
                                "No connection for account (something very strange has happened)",
                            )
                            .clone()
                            .unbounded_send(message)
                            .map_err(move |err| {
                                error!(
                                    "Error sending response to account: {} {:?}",
                                    account_id, err
                                )
                            })
                    })
            })
            .then(move |_| {
                trace!("Finished reading from pending_incoming buffer");
                Ok(())
            });
        spawn(handle_pending_incoming);

        BtpService {
            outgoing: self,
            incoming_handler_type: PhantomData,
        }
    }
}

impl<T, A> OutgoingService<A> for BtpOutgoingService<T, A>
where
    T: OutgoingService<A> + Clone,
    A: Account + 'static,
{
    type Future = BoxedIlpFuture;

    /// Send an outgoing request to one of the open connections.
    ///
    /// If there is no open connection for the Account specified in `request.to`, the
    /// request will be passed through to the `next_outgoing` handler.
    fn send_request(&mut self, request: OutgoingRequest<A>) -> Self::Future {
        let account_id = request.to.id();
        if let Some(connection) = (*self.connections.read()).get(&account_id) {
            let request_id = random::<u32>();

            // Clone the trigger so that the connections stay open until we've
            // gotten the response to our outgoing request
            let keep_connections_open = self.close_all_connections.clone();

            trace!(
                "Sending outgoing request {} to account {}",
                request_id,
                account_id
            );

            match connection.unbounded_send(ilp_packet_to_ws_message(
                request_id,
                Packet::Prepare(request.prepare),
            )) {
                Ok(_) => {
                    let (sender, receiver) = oneshot::channel();
                    (*self.pending_outgoing.lock()).insert(request_id, sender);
                    Box::new(
                        receiver
                            .then(move |result| {
                                // Drop the trigger here since we've gotten the response
                                // and don't need to keep the connections open if this was the
                                // last thing we were waiting for
                                let _ = keep_connections_open;
                                result
                            })
                            .map_err(move |err| {
                                error!(
                                    "Sending request {} to account {} failed: {:?}",
                                    request_id, account_id, err
                                );
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
                    error!(
                        "Error sending websocket message for request {} to account {}: {:?}",
                        request_id, account_id, send_error
                    );
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
            trace!(
                "No open connection for account: {}, forwarding request to the next service",
                request.to.id()
            );
            Box::new(self.next_outgoing.send_request(request))
        }
    }
}

#[derive(Clone)]
pub struct BtpService<S, T, A: Account> {
    outgoing: BtpOutgoingService<T, A>,
    incoming_handler_type: PhantomData<S>,
}

impl<S, T, A> BtpService<S, T, A>
where
    S: IncomingService<A> + Clone + Send + 'static,
    T: OutgoingService<A> + Clone,
    A: Account + 'static,
{
    /// Close all of the open WebSocket connections
    pub fn close(&self) {
        self.outgoing.close();
    }
}

impl<S, T, A> OutgoingService<A> for BtpService<S, T, A>
where
    T: OutgoingService<A> + Clone + Send + 'static,
    A: Account + 'static,
{
    type Future = BoxedIlpFuture;

    /// Send an outgoing request to one of the open connections.
    ///
    /// If there is no open connection for the Account specified in `request.to`, the
    /// request will be passed through to the `next_outgoing` handler.
    fn send_request(&mut self, request: OutgoingRequest<A>) -> Self::Future {
        self.outgoing.send_request(request)
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
