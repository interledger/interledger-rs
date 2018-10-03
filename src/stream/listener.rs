use super::crypto;
use super::packet::*;
use super::{plugin_to_channels, Connection};
use base64;
use bytes::{Bytes, BytesMut};
use futures::sync::mpsc::{unbounded, UnboundedReceiver, UnboundedSender};
use futures::Future;
use futures::{Async, Poll, Sink, Stream};
use ildcp;
use ilp::{IlpPacket, IlpPrepare, IlpReject, PacketType};
use plugin::{IlpRequest, Plugin};
use std::collections::HashMap;
use std::sync::atomic::AtomicUsize;
use std::sync::{Arc, Mutex, RwLock};
use tokio;

#[derive(Clone)]
pub struct ConnectionGenerator {
  source_account: String,
  server_secret: Bytes,
}

impl ConnectionGenerator {
  pub fn generate_address_and_secret(&self, connection_tag: &str) -> (String, Bytes) {
    let token_bytes = crypto::generate_token();
    let token = base64::encode(&token_bytes);
    let token = {
      if connection_tag.len() > 0 {
        token + "~" + connection_tag
      } else {
        token
      }
    };
    let shared_secret = crypto::generate_shared_secret_from_token(
      self.server_secret.clone(),
      Bytes::from(token.clone()),
    );
    let destination_account = format!("{}.{}", self.source_account, token);
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
  Box<dyn Fn(&IlpPrepare) -> Result<(String, Bytes), IlpReject>>;

pub struct StreamListener {
  outgoing_sender: UnboundedSender<IlpRequest>,
  incoming_receiver: UnboundedReceiver<IlpRequest>,
  source_account: String,
  // TODO do these need to be wrapped in Mutexes?
  connections: Arc<RwLock<HashMap<String, UnboundedSender<IlpRequest>>>>,
  pending_requests: Arc<Mutex<HashMap<u32, Arc<String>>>>,
  // closed_connections: Arc<Mutex<HashSet<String>>>,
  next_request_id: Arc<AtomicUsize>,
  prepare_handler: PrepareToSharedSecretGenerator,
}

impl StreamListener {
  // TODO does this need to be static?
  pub fn bind<'a, S>(
    plugin: S,
    server_secret: Bytes,
  ) -> impl Future<Item = (StreamListener, ConnectionGenerator), Error = ()> + 'a
  where
    S: Plugin<Item = IlpRequest, Error = (), SinkItem = IlpRequest, SinkError = ()> + 'static,
  {
    ildcp::get_config(plugin)
      .map_err(|(_err, _plugin)| {
        error!("Error getting ILDCP config info");
      })
      .and_then(move |(config, plugin)| {
        let (outgoing_sender, incoming_receiver) = plugin_to_channels(plugin);

        let source_account = config.client_address.clone();
        let server_secret_clone = server_secret.clone();
        let prepare_handler: Box<dyn Fn(&IlpPrepare) -> Result<(String, Bytes), IlpReject>> =
          Box::new(move |prepare| {
            let local_address = prepare
              .destination
              .clone()
              .split_off(source_account.len() + 1);
            let local_address_parts: Vec<&str> = local_address.split(".").collect();
            if local_address_parts.len() == 0 {
              warn!("Got Prepare with no Connection ID: {}", prepare.destination);
              return Err(IlpReject::new("F02", "", "", Bytes::new()));
            }
            let connection_id = local_address_parts[0];
            let shared_secret = crypto::generate_shared_secret_from_token(
              server_secret_clone.clone(),
              Bytes::from(connection_id),
            );
            Ok((connection_id.to_string(), shared_secret))
          });

        let listener = StreamListener {
          outgoing_sender,
          incoming_receiver,
          source_account: config.client_address.clone(),
          connections: Arc::new(RwLock::new(HashMap::new())),
          pending_requests: Arc::new(Mutex::new(HashMap::new())),
          // closed_connections: Arc::new(Mutex::new(HashSet::new())),
          next_request_id: Arc::new(AtomicUsize::new(1)),
          prepare_handler,
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
  ) -> impl Future<Item = StreamListener, Error = ()> + 'a
  where
    S: Plugin<Item = IlpRequest, Error = (), SinkItem = IlpRequest, SinkError = ()> + 'static,
  {

    ildcp::get_config(plugin)
      .map_err(|(_err, _plugin)| {
        error!("Error getting ILDCP config info");
      })
      .and_then(move |(config, plugin)| {
        let (outgoing_sender, incoming_receiver) = plugin_to_channels(plugin);

        let listener = StreamListener {
          outgoing_sender,
          incoming_receiver,
          source_account: config.client_address.clone(),
          connections: Arc::new(RwLock::new(HashMap::new())),
          pending_requests: Arc::new(Mutex::new(HashMap::new())),
          // closed_connections: Arc::new(Mutex::new(HashSet::new())),
          next_request_id: Arc::new(AtomicUsize::new(1)),
          prepare_handler,
        };
        Ok(listener)
      })
  }

  fn handle_new_connection(
    &mut self,
    connection_id: &str,
    shared_secret: Bytes,
    request_id: u32,
    prepare: IlpPrepare,
  ) -> Result<Option<Connection>, ()> {
    // Check that the connection is legitimate by decrypting the packet
    // Also make sure they sent us their address
    let destination_account = {
      if let Ok(stream_packet) =
        StreamPacket::from_encrypted(shared_secret.clone(), BytesMut::from(&prepare.data[..]))
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
          let data = response_packet.to_encrypted(shared_secret.clone()).unwrap();
          self
            .outgoing_sender
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
        self
          .outgoing_sender
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
    self
      .connections
      .write()
      .unwrap()
      .insert(connection_id.to_string(), incoming_tx.clone());
    let (outgoing_tx, outgoing_rx) = unbounded::<IlpRequest>();

    let connection_id = Arc::new(connection_id.to_string());
    let pending_requests = Arc::clone(&self.pending_requests);
    let connection_id_clone = Arc::clone(&connection_id);
    let outgoing_rx = outgoing_rx.inspect(move |(request_id, packet)| {
      let request_id = request_id.clone();
      let connection_id = Arc::clone(&connection_id_clone);
      if let IlpPacket::Prepare(_prepare) = packet {
        // TODO avoid storing the connection_id over and over
        pending_requests
          .lock()
          .unwrap()
          .insert(request_id, connection_id);
      }
    });
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
      shared_secret.clone(),
      self.source_account.to_string(),
      destination_account,
      true,
      Arc::clone(&self.next_request_id),
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

    return Ok(Some(conn));
  }

  fn handle_response(&mut self, request_id: u32, response: IlpPacket) {
    let pending_requests = self.pending_requests.lock().unwrap();
    if let Some(connection_id) = pending_requests.get(&request_id) {
      let connection_id = connection_id.to_string();
      let connections = self.connections.read().unwrap();
      let incoming_tx = connections.get(&connection_id).unwrap();
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
}

impl Stream for StreamListener {
  type Item = Connection;
  type Error = ();

  fn poll(&mut self) -> Poll<Option<Self::Item>, Self::Error> {
    loop {
      trace!("Polling plugin for more incoming packets");
      let next = try_ready!(self.incoming_receiver.poll());
      if next.is_none() {
        debug!("Incoming stream closed");
        return Ok(Async::Ready(None));
      }
      let (request_id, packet) = next.unwrap();

      // Forward requests to the right Connection
      // Also check if we got a new incoming Connection
      match packet {
        IlpPacket::Prepare(prepare) => {
          // Handle new Connections or figure out which existing Connection to forward the Prepare to

          // First, generate the shared_secret
          let (connection_id, shared_secret) = {
            match (self.prepare_handler)(&prepare) {
              Ok((connection_id, shared_secret)) => (connection_id, shared_secret),
              Err(reject) => {
                trace!("Rejecting request {} (unable to generate shared secret or alternate prepare handler rejected the packet)", request_id);
                self
                  .outgoing_sender
                  .unbounded_send((request_id, IlpPacket::Reject(reject)))
                  .map_err(|_| {
                    error!("Error sending reject");
                  })?;
                continue;
              }
            }
          };

          // TODO check if the connection was already closed
          // if self.closed_connections.contains(connection_id){
          //   warn!("Got Prepare for closed connection {}", prepare.destination);
          //   self.outgoing_sender.unbounded_send((request_id, IlpPacket::Reject(IlpReject::new("F02", "", "", Bytes::new()))))
          //     .map_err(|_| {
          //       error!("Error sending reject");
          //     })?;
          //   return Ok(Async::NotReady);
          // }

          let is_new_connection = !self
            .connections
            .read()
            .unwrap()
            .contains_key(&connection_id);
          if is_new_connection {
            if let Ok(Some(connection)) =
              self.handle_new_connection(&connection_id, shared_secret, request_id, prepare)
            {
              return Ok(Async::Ready(Some(connection)));
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
            let connections = self.connections.read().unwrap();
            let channel = connections.get(&connection_id).unwrap();
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
        _ => {
          debug!("Ignoring unknown ILP packet");
          continue;
        }
      }
    }
  }
}
