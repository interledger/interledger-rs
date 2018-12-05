use super::crypto;
use super::packet::*;
use super::Error;
use super::{plugin_to_channels, Connection};
use base64;
use bytes::{Bytes, BytesMut};
use futures::sync::mpsc::{unbounded, UnboundedReceiver, UnboundedSender};
use futures::Future;
use futures::{Async, Poll, Sink, Stream};
use ildcp;
use ilp::{IlpPacket, IlpPrepare, IlpReject, PacketType};
use parking_lot::{Mutex, RwLock};
use plugin::{IlpRequest, Plugin};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use stream_cancel::{Trigger, Valved};
use tokio;

lazy_static! {
    static ref TAG_ENCRYPTION_KEY_STRING: &'static [u8] = b"ilp_stream_tag_encryption_aes";
}

fn encrypt_tag(server_secret: &[u8], tag: &str) -> String {
    let key = crypto::hmac_sha256(server_secret, &TAG_ENCRYPTION_KEY_STRING);
    let encrypted = crypto::encrypt(&key[..], BytesMut::from(tag.as_bytes()));
    base64::encode_config(&encrypted[..], base64::URL_SAFE_NO_PAD)
}

fn decrypt_tag(server_secret: &[u8], encrypted: &str) -> String {
    let key = crypto::hmac_sha256(server_secret, &TAG_ENCRYPTION_KEY_STRING);
    let decoded =
        base64::decode_config(encrypted, base64::URL_SAFE_NO_PAD).unwrap_or_else(|_| Vec::new());
    let decrypted =
        crypto::decrypt(&key[..], BytesMut::from(&decoded[..])).unwrap_or_else(|_| BytesMut::new());
    let decrypted_vec = decrypted.freeze().to_vec();
    String::from_utf8(decrypted_vec).unwrap_or_else(|_| String::new())
}

#[derive(Clone)]
pub struct ConnectionGenerator {
    source_account: String,
    server_secret: Bytes,
}

impl ConnectionGenerator {
    pub fn new<S, B>(source_account: S, server_secret: B) -> Self
    where
        String: From<S>,
        Bytes: From<B>,
    {
        ConnectionGenerator {
            source_account: String::from(source_account),
            server_secret: Bytes::from(server_secret),
        }
    }

    pub fn generate_address_and_secret(&self, connection_tag: &str) -> (String, Bytes) {
        let token_bytes = crypto::generate_token();
        let token = base64::encode_config(&token_bytes, base64::URL_SAFE_NO_PAD);

        // TODO include the connection_tag in the shared secret
        // so removing it would cause the packets to be rejected
        let shared_secret =
            crypto::generate_shared_secret_from_token(&self.server_secret, &token.as_bytes());
        let destination_account = if connection_tag.is_empty() {
            format!("{}.{}", self.source_account, token)
        } else {
            let encrypted_tag = encrypt_tag(&self.server_secret, connection_tag);
            // TODO don't use the ~ so it's harder to identify which part is which from the outside
            format!("{}.{}~{}", self.source_account, token, encrypted_tag)
        };
        (destination_account, shared_secret)
    }
}

/**
 * This function takes an ILP Prepare packet and returns the
 * connection ID and shared secret generated from it.
 * If it cannot handle the packet it SHOULD return Ok(None).
 * If the handler knows the packet is for it and should be rejected,
 * it MAY return an Err with an IlpReject packet that will be sent back to the sender
 */
pub type PrepareToSharedSecretGenerator =
    Box<dyn Fn(&str, &IlpPrepare) -> Result<(String, Bytes), IlpReject> + Send + Sync>;

type ConnectionMap = HashMap<String, (UnboundedSender<IlpRequest>, Trigger, Connection)>;

pub struct StreamListener {
    outgoing_sender: UnboundedSender<IlpRequest>,
    incoming_receiver: UnboundedReceiver<IlpRequest>,
    source_account: String,
    // TODO do these need to be wrapped in Mutexes?
    connections: Arc<RwLock<ConnectionMap>>,
    pending_requests: Arc<Mutex<HashMap<u32, Arc<String>>>>,
    closed_connections: Arc<Mutex<HashSet<String>>>,
    prepare_handler: Arc<PrepareToSharedSecretGenerator>,
}

type PrepareHandler =
    Box<dyn Fn(&str, &IlpPrepare) -> Result<(String, Bytes), IlpReject> + Send + Sync>;

pub(super) fn derive_shared_secret(
    server_secret: Bytes,
    local_address: &str,
    prepare: &IlpPrepare,
) -> Result<(String, Bytes), IlpReject> {
    let local_address_parts: Vec<&str> = local_address.split('.').collect();
    if local_address_parts.is_empty() {
        warn!("Got Prepare with no Connection ID: {}", prepare.destination);
        return Err(IlpReject::new("F02", "", "", Bytes::new()));
    }
    let connection_id = local_address_parts[0];

    let split: Vec<&str> = connection_id.splitn(2, '~').collect();
    let token = split[0];
    let shared_secret =
        crypto::generate_shared_secret_from_token(&server_secret, &token.as_bytes());
    if split.len() == 1 {
        Ok((connection_id.to_string(), shared_secret))
    } else {
        let encrypted_tag = split[1];
        let decrypted = decrypt_tag(&server_secret[..], &encrypted_tag);
        // TODO don't mash these two together, just return them separately
        let connection_id = format!("{}~{}", token, decrypted);
        Ok((connection_id, shared_secret))
    }
}

impl StreamListener {
    // TODO does this need to be static?
    pub fn bind<'a, S>(
        plugin: S,
        server_secret: Bytes,
    ) -> impl Future<Item = (StreamListener, ConnectionGenerator), Error = Error> + 'a + Send + Sync
    where
        S: Plugin<Item = IlpRequest, Error = (), SinkItem = IlpRequest, SinkError = ()> + 'static,
    {
        ildcp::get_config(plugin)
            .map_err(|err| Error::ConnectionError(format!("Error connecting: {}", err)))
            .and_then(move |(config, plugin)| {
                let (outgoing_sender, incoming_receiver) = plugin_to_channels(plugin);

                let server_secret_clone = server_secret.clone();
                let prepare_handler: PrepareHandler = Box::new(move |local_address, prepare| {
                    derive_shared_secret(server_secret_clone.clone(), &local_address, &prepare)
                });

                let listener = StreamListener {
                    outgoing_sender,
                    incoming_receiver,
                    source_account: config.client_address.clone(),
                    connections: Arc::new(RwLock::new(HashMap::new())),
                    pending_requests: Arc::new(Mutex::new(HashMap::new())),
                    closed_connections: Arc::new(Mutex::new(HashSet::new())),
                    prepare_handler: Arc::new(prepare_handler),
                };

                let generator = ConnectionGenerator {
                    source_account: config.client_address,
                    server_secret,
                };

                Ok((listener, generator))
            })
    }

    pub fn bind_with_custom_prepare_handler<'a, S>(
        plugin: S,
        prepare_handler: PrepareToSharedSecretGenerator,
    ) -> impl Future<Item = StreamListener, Error = Error> + 'a
    where
        S: Plugin<Item = IlpRequest, Error = (), SinkItem = IlpRequest, SinkError = ()> + 'static,
    {
        ildcp::get_config(plugin)
            .map_err(|err| Error::ConnectionError(format!("Error connecting: {}", err)))
            .and_then(move |(config, plugin)| {
                let (outgoing_sender, incoming_receiver) = plugin_to_channels(plugin);

                let listener = StreamListener {
                    outgoing_sender,
                    incoming_receiver,
                    source_account: config.client_address.clone(),
                    connections: Arc::new(RwLock::new(HashMap::new())),
                    pending_requests: Arc::new(Mutex::new(HashMap::new())),
                    closed_connections: Arc::new(Mutex::new(HashSet::new())),
                    prepare_handler: Arc::new(prepare_handler),
                };
                Ok(listener)
            })
    }

    pub fn source_account(&self) -> String {
        self.source_account.to_string()
    }

    fn handle_new_connection(
        &mut self,
        connection_id: &str,
        shared_secret: &[u8],
        request_id: u32,
        prepare: IlpPrepare,
    ) -> Result<Option<Connection>, ()> {
        // Check that the connection is legitimate by decrypting the packet
        // Also make sure they sent us their address
        let destination_account = {
            if let Ok(stream_packet) =
                StreamPacket::from_encrypted(&shared_secret, BytesMut::from(&prepare.data[..]))
            {
                let frame = stream_packet.frames.iter().find(|frame| {
                    if let Frame::ConnectionNewAddress(_) = frame {
                        true
                    } else {
                        false
                    }
                });
                if let Some(Frame::ConnectionNewAddress(address_frame)) = frame {
                    address_frame.source_account.to_string()
                } else {
                    warn!(
                        "Got new Connection frame that did not have the sender's address {:?}",
                        stream_packet
                    );
                    let response_packet = StreamPacket {
                        sequence: stream_packet.sequence,
                        ilp_packet_type: PacketType::IlpReject,
                        prepare_amount: 0,
                        frames: vec![],
                    };
                    let data = response_packet.to_encrypted(&shared_secret);
                    self.outgoing_sender
                        .unbounded_send((
                            request_id,
                            IlpPacket::Reject(IlpReject::new("F99", "", "", data)),
                        ))
                        .map_err(|_| {
                            error!("Error sending reject");
                        })?;
                    return Ok(None);
                }
            } else {
                warn!(
                    "Got Prepare with stream packet that we cannot parse: {:?}",
                    prepare
                );
                self.outgoing_sender
                    .unbounded_send((
                        request_id,
                        IlpPacket::Reject(IlpReject::new("F02", "", "", Bytes::new())),
                    ))
                    .map_err(|_| {
                        error!("Error sending reject");
                    })?;
                return Ok(None);
            }
        };

        debug!("Got new connection with ID: {}", connection_id);

        // Set up streams to forward to/from the connection
        let (incoming_tx, incoming_rx) = unbounded::<IlpRequest>();
        let (outgoing_tx, outgoing_rx) = unbounded::<IlpRequest>();

        let connection_id = Arc::new(connection_id.to_string());
        let pending_requests = Arc::clone(&self.pending_requests);
        let connection_id_clone = Arc::clone(&connection_id);
        let outgoing_rx = outgoing_rx.inspect(move |(request_id, packet)| {
            let connection_id = Arc::clone(&connection_id_clone);
            if let IlpPacket::Prepare(_prepare) = packet {
                // TODO avoid storing the connection_id over and over
                (*pending_requests.lock()).insert(*request_id, connection_id);
            }
        });

        // Stop forwarding when the trigger is dropped (when the connection closes)
        let (trigger, outgoing_rx) = Valved::new(outgoing_rx);

        let forward_outgoing = self
            .outgoing_sender
            .clone()
            .sink_map_err(|err| {
                error!(
                    "Error forwarding packets from connection to outgoing sink {:?}",
                    err
                );
            })
            .send_all(outgoing_rx)
            .then(|_| Ok(()));
        tokio::spawn(forward_outgoing);

        let conn = Connection::new(
            outgoing_tx,
            incoming_rx,
            Bytes::from(shared_secret),
            self.source_account.to_string(),
            destination_account,
            true,
        );

        incoming_tx
            .unbounded_send((request_id, IlpPacket::Prepare(prepare)))
            .map_err(|err| {
                error!(
                    "Error sending request {} to connection {}: {:?}",
                    request_id,
                    connection_id.clone(),
                    err
                );
            })?;

        (*self.connections.write()).insert(
            connection_id.to_string(),
            (incoming_tx.clone(), trigger, conn.clone()),
        );

        Ok(Some(conn))
    }

    fn handle_response(&mut self, request_id: u32, response: IlpPacket) {
        let pending_requests = self.pending_requests.lock();
        if let Some(connection_id) = pending_requests.get(&request_id) {
            let connection_id = connection_id.to_string();
            let connections = self.connections.read();
            let (incoming_tx, _trigger, _conn) = connections.get(&connection_id).unwrap();
            trace!(
                "Sending response for request {} to connection {}",
                request_id,
                connection_id
            );
            incoming_tx
                .unbounded_send((request_id, response))
                .or_else(|err| -> Result<(), ()> {
                    error!(
                        "Error sending response to connection: {} {:?}",
                        connection_id, err
                    );
                    Ok(())
                })
                .unwrap();
        } else {
            warn!(
                "Ignoring response packet that did not correspond to outgoing request: {} {:?}",
                request_id, response
            );
        }
    }

    fn check_for_closed_connections(&self) {
        let mut connections_to_remove: Vec<String> = Vec::new();
        for (id, (_sender, _trigger, conn)) in (*self.connections.read()).iter() {
            if conn.is_closed() {
                connections_to_remove.push(id.to_string());
            }
        }

        if !connections_to_remove.is_empty() {
            let mut connections = self.connections.write();
            let mut closed_connections = self.closed_connections.lock();
            for id in connections_to_remove.iter() {
                debug!("Connection {} was closed, removing entry", id);
                let entry = connections.remove(id.as_str());
                if let Some((mut sender, trigger, _conn)) = entry {
                    sender.close().unwrap();
                    drop(trigger);
                }
                closed_connections.insert(id.to_string());
            }
        }
    }
}

impl Stream for StreamListener {
    type Item = (String, Connection);
    type Error = ();

    fn poll(&mut self) -> Poll<Option<Self::Item>, Self::Error> {
        loop {
            trace!("Polling plugin for more incoming packets");

            // TODO timeout connections so they close even if we don't get another packet
            self.check_for_closed_connections();

            let next = try_ready!(self.incoming_receiver.poll());
            if next.is_none() {
                debug!("Incoming stream closed");
                return Ok(Async::Ready(None));
            }
            let (request_id, packet) = next.unwrap();
            trace!("Handling packet with request ID {}", request_id);

            // Forward requests to the right Connection
            // Also check if we got a new incoming Connection
            match packet {
                IlpPacket::Prepare(prepare) => {
                    // Handle new Connections or figure out which existing Connection to forward the Prepare to

                    // First, generate the shared_secret
                    let local_address = prepare
                        .destination
                        .clone()
                        .split_off(self.source_account.len() + 1);
                    let (connection_id, shared_secret) = {
                        match (self.prepare_handler)(&local_address, &prepare) {
                            Ok((connection_id, shared_secret)) => (connection_id, shared_secret),
                            Err(reject) => {
                                trace!("Rejecting request {} (unable to generate shared secret or alternate prepare handler rejected the packet)", request_id);
                                self.outgoing_sender
                                    .unbounded_send((request_id, IlpPacket::Reject(reject)))
                                    .map_err(|_| {
                                        error!("Error sending reject");
                                    })?;
                                continue;
                            }
                        }
                    };

                    // TODO check if the connection was already closed
                    if self.closed_connections.lock().contains(&connection_id) {
                        warn!("Got Prepare for closed connection {}", prepare.destination);
                        self.outgoing_sender
                            .unbounded_send((
                                request_id,
                                IlpPacket::Reject(IlpReject::new("F02", "", "", Bytes::new())),
                            ))
                            .map_err(|_| {
                                error!("Error sending reject");
                            })?;
                        return Ok(Async::NotReady);
                    }

                    let is_new_connection = !self.connections.read().contains_key(&connection_id);
                    if is_new_connection {
                        if let Ok(Some(connection)) = self.handle_new_connection(
                            &connection_id,
                            &shared_secret,
                            request_id,
                            prepare,
                        ) {
                            return Ok(Async::Ready(Some((connection_id.to_string(), connection))));
                        } else {
                            continue;
                        }
                    } else {
                        trace!(
                            "Sending Prepare {} to connection {}",
                            request_id,
                            connection_id
                        );
                        // Send the packet to the Connection
                        let connections = self.connections.read();
                        let (channel, _trigger, _conn) = connections.get(&connection_id).unwrap();
                        channel
                            .unbounded_send((request_id, IlpPacket::Prepare(prepare)))
                            .unwrap();
                        continue;
                    }
                }
                IlpPacket::Fulfill(fulfill) => {
                    self.handle_response(request_id, IlpPacket::Fulfill(fulfill));
                    continue;
                }
                IlpPacket::Reject(reject) => {
                    self.handle_response(request_id, IlpPacket::Reject(reject));
                    continue;
                }
            }
        }
    }
}
