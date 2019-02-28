use super::packet::*;
use super::service::{ilp_packet_to_ws_message, parse_ilp_packet, BtpService, IlpResultChannel};
use super::BtpStore;
use futures::{
    future::{join_all, ok, Either},
    sync::mpsc::unbounded,
    Future, Sink, Stream,
};
use hashbrown::HashMap;
use interledger_packet::Packet;
use interledger_service::*;
use parking_lot::{Mutex, RwLock};
use rand::random;
use std::io::{Error as IoError, ErrorKind};
use std::iter::{FromIterator, IntoIterator};
use std::sync::Arc;
use tokio;
use tokio_tungstenite::connect_async;
use tungstenite::{error::Error as WebSocketError, Message};
use url::{ParseError, Url};

pub fn parse_btp_url(uri: &str) -> Result<Url, ParseError> {
    let uri = if uri.starts_with("btp+") {
        uri.split_at(4).1
    } else {
        uri
    };
    Url::parse(uri)
}

pub fn connect_client<S, T>(
    next: S,
    store: T,
    accounts: impl IntoIterator<Item = AccountId>,
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
          debug!("Connecting to {}", url);
        connect_async(url.clone()).map_err(|_err| ())
        .and_then(move |(connection, _)| {
            debug!("Connected to {}, sending auth packet", url);
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
