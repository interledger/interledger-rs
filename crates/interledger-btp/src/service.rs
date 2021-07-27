use super::{packet::*, BtpAccount};
use async_trait::async_trait;
use bytes::BytesMut;
use futures::{
    channel::{
        mpsc::{unbounded, UnboundedReceiver, UnboundedSender},
        oneshot,
    },
    future, FutureExt, Sink, Stream, StreamExt,
};
use interledger_packet::{Address, ErrorCode, Fulfill, Packet, Prepare, Reject, RejectBuilder};
use interledger_service::*;
use once_cell::sync::Lazy;
use parking_lot::{Mutex, RwLock};
use rand::random;
use std::collections::HashMap;
use std::{convert::TryFrom, iter::IntoIterator, marker::PhantomData, sync::Arc, time::Duration};
use stream_cancel::{Trigger, Valve};
use tokio::time;
use tokio_tungstenite::tungstenite::Message;
use tracing::{debug, error, trace, warn};
use uuid::Uuid;

const PING_INTERVAL: u64 = 30; // seconds

static PING: Lazy<Message> = Lazy::new(|| Message::Ping(Vec::with_capacity(0)));
static PONG: Lazy<Message> = Lazy::new(|| Message::Pong(Vec::with_capacity(0)));

// Return a Reject timeout if the outgoing message future does not complete
// within this timeout. This will probably happen if the peer closed the websocket
// with us
const SEND_MSG_TIMEOUT: Duration = Duration::from_secs(30);

type IlpResultChannel = oneshot::Sender<Result<Fulfill, Reject>>;
type IncomingRequestBuffer<A> = UnboundedReceiver<(A, u32, Prepare)>;

/// The BtpOutgoingService wraps all BTP/WebSocket connections that come
/// in on the given address. It implements OutgoingService for sending
/// outgoing ILP Prepare packets over one of the connected BTP connections.
/// Calling `handle_incoming` with an `IncomingService` will turn the returned
/// BtpOutgoingService into a bidirectional handler.
/// The separation is designed to enable the returned BtpOutgoingService to be passed
/// to another service like the Router, and _then_ for the Router to be passed as the
/// IncomingService to the BTP server.
#[derive(Clone)]
pub struct BtpOutgoingService<O, A: Account> {
    ilp_address: Address,
    /// Outgoing messages for the receiver of the websocket indexed by account uid
    connections: Arc<RwLock<HashMap<Uuid, UnboundedSender<Message>>>>,
    pending_outgoing: Arc<Mutex<HashMap<u32, IlpResultChannel>>>,
    pending_incoming: Arc<Mutex<Option<IncomingRequestBuffer<A>>>>,
    incoming_sender: UnboundedSender<(A, u32, Prepare)>,
    next: O,
    close_all_connections: Arc<Mutex<Option<Trigger>>>,
    stream_valve: Arc<Valve>,
}

/// Handle the packets based on whether they are an incoming request or a response to something we sent.
///  a. If it's a Prepare packet, it gets buffered in the incoming_sender channel which will get consumed
///     once an incoming handler is added
///  b. If it's a Fulfill/Reject packet, it gets added to the pending_outgoing hashmap which gets consumed
///     by the outgoing service implementation immediately
/// incoming_sender.unbounded_send basically sends data to the self.incoming_receiver
/// to be consumed when we setup the incoming handler
/// Set up a listener to handle incoming packets from the WebSocket connection
#[inline]
async fn handle_message<A: BtpAccount>(
    message: Message,
    tx_clone: UnboundedSender<Message>,
    account: A,
    pending_requests: Arc<Mutex<HashMap<u32, IlpResultChannel>>>,
    incoming_sender: UnboundedSender<(A, u32, Prepare)>,
) {
    if message.is_binary() {
        match parse_ilp_packet(message) {
            // Queues up the prepare packet
            Ok((request_id, Packet::Prepare(prepare))) => {
                trace!(
                    "Got incoming Prepare packet on request ID: {} {:?}",
                    request_id,
                    prepare
                );
                let _ = incoming_sender
                    .unbounded_send((account, request_id, prepare))
                    .map_err(|err| error!("Unable to buffer incoming request: {:?}", err));
            }
            // Sends the fulfill/reject to the outgoing service
            Ok((request_id, Packet::Fulfill(fulfill))) => {
                trace!("Got fulfill response to request id {}", request_id);
                if let Some(channel) = (*pending_requests.lock()).remove(&request_id) {
                    let _ = channel.send(Ok(fulfill)).map_err(|fulfill| error!("Error forwarding Fulfill packet back to the Future that sent the Prepare: {:?}", fulfill));
                } else {
                    warn!(
                        "Got Fulfill packet that does not match an outgoing Prepare we sent: {:?}",
                        fulfill
                    );
                }
            }
            Ok((request_id, Packet::Reject(reject))) => {
                trace!("Got reject response to request id {}", request_id);
                if let Some(channel) = (*pending_requests.lock()).remove(&request_id) {
                    let _ = channel.send(Err(reject)).map_err(|reject| error!("Error forwarding Reject packet back to the Future that sent the Prepare: {:?}", reject));
                } else {
                    warn!(
                        "Got Reject packet that does not match an outgoing Prepare we sent: {:?}",
                        reject
                    );
                }
            }
            Err(_) => {
                debug!("Unable to parse ILP packet from BTP packet (if this is the first time this appears, the packet was probably the auth response)");
                // TODO Send error back
            }
        }
    } else if message.is_ping() {
        trace!("Responding to Ping message from account {}", account.id());
        // Writes back the PONG to the websocket
        let _ = tx_clone
            .unbounded_send(PONG.clone())
            .map_err(|err| error!("Error sending Pong message back: {:?}", err));
    }
}

impl<O, A> BtpOutgoingService<O, A>
where
    O: OutgoingService<A> + Clone,
    A: BtpAccount + Send + Sync + 'static,
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

    /// Deletes the websocket associated with the provided `account_id`
    pub fn close_connection(&self, account_id: &Uuid) {
        self.connections.write().remove(account_id);
    }

    /// Close all of the open WebSocket connections
    // TODO is there some more automatic way of knowing when we should close the connections?
    // The problem is that the WS client can be a server too, so it's not clear when we are done with it
    pub fn close(&self) {
        debug!("Closing all WebSocket connections");
        self.close_all_connections.lock().take();
    }

    // Set up a WebSocket connection so that outgoing Prepare packets can be sent to it,
    // incoming Prepare packets are buffered in a channel (until an IncomingService is added
    // via the handle_incoming method), and ILP Fulfill and Reject packets will be
    // sent back to the Future that sent the outgoing request originally.
    pub(crate) fn add_connection(
        &self,
        account: A,
        ws_stream: impl Stream<Item = Message> + Sink<Message> + Send + 'static,
    ) {
        let account_id = account.id();
        // Set up a channel to forward outgoing packets to the WebSocket connection
        let (client_tx, client_rx) = unbounded();
        let (write, read) = ws_stream.split();
        let (close_connection, valve) = Valve::new();

        // tx -> rx -> write -> our peer
        // Responsible mainly for responding to Pings
        let write_to_ws = client_rx.map(Ok).forward(write).then(move |_| {
            async move {
                debug!(
                    "Finished forwarding to WebSocket stream for account: {}",
                    account_id
                );
                // When this is dropped, the read valve will close
                drop(close_connection);
                Ok::<(), ()>(())
            }
        });
        tokio::spawn(write_to_ws);

        // Process incoming messages depending on their type
        let pending_outgoing = self.pending_outgoing.clone();
        let incoming_sender = self.incoming_sender.clone();
        let client_tx_clone = client_tx.clone();
        let handle_message_fn = move |msg: Message| {
            handle_message(
                msg,
                client_tx_clone.clone(),
                account.clone(),
                pending_outgoing.clone(),
                incoming_sender.clone(),
            )
        };

        // Close connections trigger
        let read = valve.wrap(read); // close when `write_to_ws` calls `drop(connection)`
        let read = self.stream_valve.wrap(read);
        let read_from_ws = read.for_each(handle_message_fn).then(move |_| async move {
            debug!(
                "Finished reading from WebSocket stream for account: {}",
                account_id
            );
            Ok::<(), ()>(())
        });
        tokio::spawn(read_from_ws);

        // Send pings every PING_INTERVAL until the connection closes (when `drop(close_connection)` is called)
        // or the Service is dropped (which will implicitly drop `close_all_connections`, closing the stream_valve)
        let tx_clone = client_tx.clone();

        let ping_interval = time::interval(Duration::from_secs(PING_INTERVAL));
        let ping_stream = tokio_stream::wrappers::IntervalStream::new(ping_interval);
        let repeat_until_service_drops = self.stream_valve.wrap(ping_stream);
        let send_pings = valve.wrap(repeat_until_service_drops).for_each(move |_| {
            // For each tick send a ping
            if let Err(err) = tx_clone.unbounded_send(PING.clone()) {
                warn!(
                    "Error sending Ping on connection to account {}: {:?}",
                    account_id, err
                );
            }
            future::ready(())
        });
        tokio::spawn(send_pings);

        // Save the sender side of the channel so we have a way to forward outgoing requests to the WebSocket
        self.connections.write().insert(account_id, client_tx);
    }

    /// Convert this BtpOutgoingService into a bidirectional BtpService by adding a handler for incoming requests.
    /// This will automatically pull all incoming Prepare packets from the channel buffer and call the IncomingService with them.
    pub async fn handle_incoming<I>(self, incoming_handler: I) -> BtpService<I, O, A>
    where
        I: IncomingService<A> + Clone + Send + 'static,
    {
        // Any connections that were added to the BtpOutgoingService will just buffer
        // the incoming Prepare packets they get in self.pending_incoming
        // Now that we're adding an incoming handler, this will spawn a task to read
        // all Prepare packets from the buffer, handle them, and send the responses back
        let connections_clone = self.connections.clone();
        let mut handle_pending_incoming = self
            .pending_incoming
            .lock()
            .take()
            .expect("handle_incoming can only be called once");
        let handle_pending_incoming_fut = async move {
            while let Some((account, request_id, prepare)) = handle_pending_incoming.next().await {
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
                let mut handler = incoming_handler.clone();
                let packet = match handler.handle_request(request).await {
                    Ok(fulfill) => Packet::Fulfill(fulfill),
                    Err(reject) => Packet::Reject(reject),
                };

                if let Some(connection) = connections_clone.clone().read().get(&account_id) {
                    let message = ilp_packet_to_ws_message(request_id, packet);
                    let _ = connection.unbounded_send(message).map_err(move |err| {
                        error!(
                            "Error sending response to account: {} {:?}",
                            account_id, err
                        )
                    });
                } else {
                    error!(
                        "Error sending response to account: {}, connection was closed. {:?}",
                        account_id, packet
                    );
                }
            }

            trace!("Finished reading from pending_incoming buffer");
            Ok::<(), ()>(())
        };

        tokio::spawn(handle_pending_incoming_fut);

        BtpService {
            outgoing: self,
            incoming_handler_type: PhantomData,
        }
    }
}

#[async_trait]
impl<O, A> OutgoingService<A> for BtpOutgoingService<O, A>
where
    O: OutgoingService<A> + Send + Sync + Clone + 'static,
    A: BtpAccount + Send + Sync + Clone + 'static,
{
    /// Send an outgoing request to one of the open connections.
    ///
    /// If there is no open connection for the Account specified in `request.to`, the
    /// request will be passed through to the `next` handler.
    async fn send_request(&mut self, request: OutgoingRequest<A>) -> IlpResult {
        let account_id = request.to.id();

        let found = self.connections.read().get(&account_id).cloned();

        if let Some(connection) = found {
            let request_id = random::<u32>();
            let ilp_address = self.ilp_address.clone();

            // Clone the trigger so that the connections stay open until we've
            // gotten the response to our outgoing request
            let keep_connections_open = self.close_all_connections.clone();

            trace!(
                "Sending outgoing request {} to {} ({})",
                request_id,
                request.to.username(),
                account_id
            );

            // Connection is an unbounded sender which sends to the rx that
            // forwards to the sink which sends the data over
            match connection.unbounded_send(ilp_packet_to_ws_message(
                request_id,
                Packet::Prepare(request.prepare),
            )) {
                Ok(_) => {
                    let (sender, receiver) = oneshot::channel();
                    (*self.pending_outgoing.lock()).insert(request_id, sender);

                    // Wrap the receiver with a timeout to ensure we do not
                    // wait too long if the other party has disconnected
                    // FIXME: this causes the test case to take 30s
                    let result = tokio::time::timeout(SEND_MSG_TIMEOUT, receiver).await;

                    let result = match result {
                        Ok(packet) => packet,
                        Err(err) => {
                            error!("Request timed out. Did the peer disconnect? Err: {}", err);
                            // Assume that such a long timeout means that the peer closed their
                            // connection with us, so we'll remove the pending request and the websocket
                            (*self.pending_outgoing.lock()).remove(&request_id);
                            self.close_connection(&request.to.id());

                            return Err(RejectBuilder {
                                code: ErrorCode::R00_TRANSFER_TIMED_OUT,
                                message: &[],
                                triggered_by: Some(&ilp_address),
                                data: &[],
                            }
                            .build());
                        }
                    };

                    // Drop the trigger here since we've gotten the response
                    // and don't need to keep the connections open if this was the
                    // last thing we were waiting for
                    let _ = keep_connections_open;
                    match result {
                        // This can be either a reject or a fulfill packet
                        Ok(packet) => packet,
                        Err(err) => {
                            error!(
                                "Sending request {} to account {} failed: {:?}",
                                request_id, account_id, err
                            );
                            Err(RejectBuilder {
                                code: ErrorCode::T00_INTERNAL_ERROR,
                                message: &[],
                                triggered_by: Some(&ilp_address),
                                data: &[],
                            }
                            .build())
                        }
                    }
                }
                Err(send_error) => {
                    error!(
                        "Error sending websocket message for request {} to account {}: {:?}",
                        request_id, account_id, send_error
                    );
                    Err(RejectBuilder {
                        code: ErrorCode::T00_INTERNAL_ERROR,
                        message: &[],
                        triggered_by: Some(&ilp_address),
                        data: &[],
                    }
                    .build())
                }
            }
        } else {
            if request.to.get_ilp_over_btp_url().is_some()
                || request.to.get_ilp_over_btp_outgoing_token().is_some()
            {
                trace!(
                    "No open connection for account: {}, forwarding request to the next service",
                    request.to.username()
                );
            }
            self.next.send_request(request).await
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
    A: BtpAccount + Send + Sync + 'static,
{
    /// Close all of the open WebSocket connections
    pub fn close(&self) {
        self.outgoing.close();
    }

    pub fn close_connection(&self, account_id: &Uuid) {
        self.outgoing.close_connection(account_id);
    }
}

#[async_trait]
impl<I, O, A> OutgoingService<A> for BtpService<I, O, A>
where
    I: Send, // This is a async/await requirement
    O: OutgoingService<A> + Send + Sync + Clone + 'static,
    A: BtpAccount + Send + Sync + Clone + 'static,
{
    /// Send an outgoing request to one of the open connections.
    ///
    /// If there is no open connection for the Account specified in `request.to`, the
    /// request will be passed through to the `next` handler.
    async fn send_request(&mut self, request: OutgoingRequest<A>) -> IlpResult {
        self.outgoing.send_request(request).await
    }
}

#[allow(clippy::cognitive_complexity)]
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
        if let Ok(packet) = Packet::try_from(BytesMut::from(ilp_data.as_slice())) {
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
    let (data, is_response) = match packet {
        Packet::Prepare(prepare) => (BytesMut::from(prepare).to_vec(), false),
        Packet::Fulfill(fulfill) => (BytesMut::from(fulfill).to_vec(), true),
        Packet::Reject(reject) => (BytesMut::from(reject).to_vec(), true),
    };
    let btp_packet = if is_response {
        BtpMessage {
            request_id,
            protocol_data: vec![ProtocolData {
                protocol_name: "ilp".into(),
                content_type: ContentType::ApplicationOctetStream,
                data,
            }],
        }
        .to_bytes()
    } else {
        BtpResponse {
            request_id,
            protocol_data: vec![ProtocolData {
                protocol_name: "ilp".into(),
                content_type: ContentType::ApplicationOctetStream,
                data,
            }],
        }
        .to_bytes()
    };
    Message::binary(btp_packet)
}
