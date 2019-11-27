use super::{packet::*, BtpAccount};
use bytes::BytesMut;
use futures::{
    future::err,
    sync::mpsc::{unbounded, UnboundedReceiver, UnboundedSender},
    sync::oneshot,
    Future, Sink, Stream,
};
use interledger_packet::{Address, ErrorCode, Fulfill, Packet, Prepare, Reject, RejectBuilder};
use interledger_service::*;
use log::{debug, error, trace, warn};
use parking_lot::{Mutex, RwLock};
use rand::random;
use std::collections::HashMap;
use std::{
    convert::TryFrom, error::Error, fmt, io, iter::IntoIterator, marker::PhantomData, sync::Arc,
    time::Duration,
};
use stream_cancel::{Trigger, Valve};
use tokio_executor::spawn;
use tokio_timer::Interval;
use tungstenite::Message;
use uuid::Uuid;
use warp;

const PING_INTERVAL: u64 = 30; // seconds

type IlpResultChannel = oneshot::Sender<Result<Fulfill, Reject>>;
type IncomingRequestBuffer<A> = UnboundedReceiver<(A, u32, Prepare)>;

#[derive(Debug)]
pub enum WsError {
    Tungstenite(tungstenite::Error),
    Warp(warp::Error),
}

impl fmt::Display for WsError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            WsError::Tungstenite(err) => err.fmt(f),
            WsError::Warp(err) => err.fmt(f),
        }
    }
}

impl Error for WsError {}

impl From<tungstenite::Error> for WsError {
    fn from(err: tungstenite::Error) -> Self {
        WsError::Tungstenite(err)
    }
}

impl From<warp::Error> for WsError {
    fn from(err: warp::Error) -> Self {
        WsError::Warp(err)
    }
}

/// A container for BTP/WebSocket connections that implements OutgoingService
/// for sending outgoing ILP Prepare packets over one of the connected BTP connections.
#[derive(Clone)]
pub struct BtpOutgoingService<O, A: Account> {
    // TODO support multiple connections per account
    ilp_address: Address,
    connections: Arc<RwLock<HashMap<Uuid, UnboundedSender<Message>>>>,
    pending_outgoing: Arc<Mutex<HashMap<u32, IlpResultChannel>>>,
    pending_incoming: Arc<Mutex<Option<IncomingRequestBuffer<A>>>>,
    incoming_sender: UnboundedSender<(A, u32, Prepare)>,
    next: O,
    close_all_connections: Arc<Mutex<Option<Trigger>>>,
    stream_valve: Arc<Valve>,
}

impl<O, A> BtpOutgoingService<O, A>
where
    O: OutgoingService<A> + Clone,
    A: BtpAccount + 'static,
{
    pub fn new(ilp_address: Address, next: O) -> Self {
        let (incoming_sender, incoming_receiver) = unbounded();
        let (close_all_connections, stream_valve) = Valve::new();
        BtpOutgoingService {
            ilp_address,
            connections: Arc::new(RwLock::new(HashMap::new())),
            pending_outgoing: Arc::new(Mutex::new(HashMap::new())),
            pending_incoming: Arc::new(Mutex::new(Some(incoming_receiver))),
            incoming_sender,
            next,
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
    pub(crate) fn add_connection(
        &self,
        account: A,
        connection: impl Stream<Item = Message, Error = WsError>
            + Sink<SinkItem = Message, SinkError = WsError>
            + Send
            + 'static,
    ) {
        let account_id = account.id();

        // Set up a channel to forward outgoing packets to the WebSocket connection
        let (tx, rx) = unbounded();
        let (sink, stream) = connection.split();
        let (close_connection, valve) = Valve::new();
        let stream = valve.wrap(stream);
        let stream = self.stream_valve.wrap(stream);
        let forward_to_connection = sink
            .send_all(rx.map_err(|_err| {
                WsError::Tungstenite(io::Error::from(io::ErrorKind::ConnectionAborted).into())
            }))
            .then(move |_| {
                debug!(
                    "Finished forwarding to WebSocket stream for account: {}",
                    account_id
                );
                drop(close_connection);
                Ok(())
            });

        // Send pings every PING_INTERVAL until the connection closes or the Service is dropped
        let tx_clone = tx.clone();
        let send_pings = valve
            .wrap(
                self.stream_valve
                    .wrap(Interval::new_interval(Duration::from_secs(PING_INTERVAL))),
            )
            .map_err(|err| {
                warn!("Timer error on Ping interval: {:?}", err);
            })
            .for_each(move |_| {
                if let Err(err) = tx_clone.unbounded_send(Message::Ping(Vec::with_capacity(0))) {
                    warn!(
                        "Error sending Ping on connection to account {}: {:?}",
                        account_id, err
                    );
                }
                Ok(())
            });
        spawn(send_pings);

        // Set up a listener to handle incoming packets from the WebSocket connection
        // TODO do we need all this cloning?
        let pending_requests = self.pending_outgoing.clone();
        let incoming_sender = self.incoming_sender.clone();
        let tx_clone = tx.clone();
        let handle_incoming = stream.map_err(move |err| error!("Error reading from WebSocket stream for account {}: {:?}", account_id, err)).for_each(move |message| {
          // Handle the packets based on whether they are an incoming request or a response to something we sent
          if message.is_binary() {
              match parse_ilp_packet(message) {
                Ok((request_id, Packet::Prepare(prepare))) => {
                    trace!("Got incoming Prepare packet on request ID: {} {:?}", request_id, prepare);
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
          } else if message.is_ping() {
              trace!("Responding to Ping message from account {}", account.id());
              tx_clone.unbounded_send(Message::Pong(Vec::new())).map_err(|err| error!("Error sending Pong message back: {:?}", err))
          } else {
              Ok(())
          }
        }).then(move |result| {
            debug!("Finished reading from WebSocket stream for account: {}", account_id);
            result
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
    pub fn handle_incoming<I>(self, incoming_handler: I) -> BtpService<I, O, A>
    where
        I: IncomingService<A> + Clone + Send + 'static,
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
                    "Handling incoming request {} from account: {} (id: {})",
                    request_id,
                    request.from.username(),
                    request.from.id()
                );
                incoming_handler_clone
                    .handle_request(request)
                    .then(move |result| {
                        let packet = match result {
                            Ok(fulfill) => Packet::Fulfill(fulfill),
                            Err(reject) => Packet::Reject(reject),
                        };
                        if let Some(connection) = connections_clone
                            .read()
                            .get(&account_id) {
                            let message = ilp_packet_to_ws_message(request_id, packet);
                            connection
                                .clone()
                                .unbounded_send(message)
                                .map_err(move |err| {
                                    error!(
                                        "Error sending response to account: {} {:?}",
                                        account_id, err
                                    )
                                })
                            } else {
                                error!("Error sending response to account: {}, connection was closed. {:?}", account_id, packet);
                                Err(())
                            }
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

impl<O, A> OutgoingService<A> for BtpOutgoingService<O, A>
where
    O: OutgoingService<A> + Clone,
    A: BtpAccount + 'static,
{
    type Future = BoxedIlpFuture;

    /// Send an outgoing request to one of the open connections.
    ///
    /// If there is no open connection for the Account specified in `request.to`, the
    /// request will be passed through to the `next` handler.
    fn send_request(&mut self, request: OutgoingRequest<A>) -> Self::Future {
        let account_id = request.to.id();
        if let Some(connection) = (*self.connections.read()).get(&account_id) {
            let request_id = random::<u32>();
            let ilp_address = self.ilp_address.clone();

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
                                    triggered_by: Some(&ilp_address),
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
                        triggered_by: Some(&ilp_address),
                        data: &[],
                    }
                    .build();
                    Box::new(err(reject))
                }
            }
        } else if request.to.get_ilp_over_btp_url().is_some()
            || request.to.get_ilp_over_btp_outgoing_token().is_some()
        {
            trace!(
                "No open connection for account: {}, forwarding request to the next service",
                request.to.id()
            );
            Box::new(self.next.send_request(request))
        } else {
            Box::new(self.next.send_request(request))
        }
    }
}

#[derive(Clone)]
pub struct BtpService<I, O, A: Account> {
    outgoing: BtpOutgoingService<O, A>,
    incoming_handler_type: PhantomData<I>,
}

impl<I, O, A> BtpService<I, O, A>
where
    I: IncomingService<A> + Clone + Send + 'static,
    O: OutgoingService<A> + Clone,
    A: BtpAccount + 'static,
{
    /// Close all of the open WebSocket connections
    pub fn close(&self) {
        self.outgoing.close();
    }
}

impl<I, O, A> OutgoingService<A> for BtpService<I, O, A>
where
    O: OutgoingService<A> + Clone + Send + 'static,
    A: BtpAccount + 'static,
{
    type Future = BoxedIlpFuture;

    /// Send an outgoing request to one of the open connections.
    ///
    /// If there is no open connection for the Account specified in `request.to`, the
    /// request will be passed through to the `next` handler.
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
