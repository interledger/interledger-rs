use super::crypto::{generate_condition, random_condition};
use super::packet::*;
use super::StreamPacket;
use bytes::Bytes;
use chrono::{Duration, Utc};
use futures::future::ok;
use futures::stream::poll_fn;
use futures::sync::mpsc::{unbounded, UnboundedReceiver, UnboundedSender};
use futures::{Async, Future, Poll, Sink, Stream};
use ilp::{IlpFulfill, IlpPacket, IlpPrepare, IlpReject, PacketType};
use num_bigint::BigUint;
use plugin::IlpRequest;
use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use tokio;

pub struct DataMoneyStream {
  id: u64,
  conn: Connection,
}

impl DataMoneyStream {
  pub fn send_money(&mut self, amount: u64) -> impl Future<Item = (), Error = ()> {
    self.conn.send_money(self.id, amount)
  }
}

#[derive(Clone)]
pub struct Connection {
  state: Arc<RwLock<ConnectionState>>,
  outgoing: UnboundedSender<IlpRequest>,
  incoming: Arc<Mutex<UnboundedReceiver<IlpRequest>>>,
  incoming_queue: UnboundedSender<IlpRequest>,
  shared_secret: Bytes,
  source_account: Arc<String>,
  destination_account: Arc<String>,
  next_stream_id: Arc<AtomicUsize>,
  next_packet_sequence: Arc<AtomicUsize>,
  next_request_id: Arc<AtomicUsize>,
}

#[derive(PartialEq, Debug)]
enum ConnectionState {
  Opening,
  Open,
  Closed,
  Closing,
}

impl Connection {
  pub fn new(
    outgoing: UnboundedSender<IlpRequest>,
    incoming: UnboundedReceiver<IlpRequest>,
    shared_secret: Bytes,
    source_account: String,
    destination_account: String,
    is_server: bool,
  ) -> Self {
    let next_stream_id = match is_server {
      true => 2,
      false => 1,
    };

    let (sender, receiver) = unbounded::<IlpRequest>();
    let queue_incoming = incoming.forward(sender.clone().sink_map_err(|err| {
      error!("Error forwarding request to queue {:?}", err);
    }))
    .then(|_| Ok(()));
    tokio::spawn(queue_incoming);

    Connection {
      state: Arc::new(RwLock::new(ConnectionState::Opening)),
      outgoing,
      incoming: Arc::new(Mutex::new(receiver)),
      incoming_queue: sender.clone(),
      shared_secret,
      source_account: Arc::new(source_account),
      destination_account: Arc::new(destination_account),
      next_stream_id: Arc::new(AtomicUsize::new(next_stream_id)),
      next_packet_sequence: Arc::new(AtomicUsize::new(1)),
      next_request_id: Arc::new(AtomicUsize::new(1)),
    }
  }

  pub fn send_money(&mut self, stream_id: u64, amount: u64) -> impl Future<Item = (), Error = ()> {
    // TODO figure out max packet amount

    let stream_packet = StreamPacket {
      sequence: self.next_packet_sequence.fetch_add(1, Ordering::SeqCst) as u64,
      ilp_packet_type: PacketType::IlpPrepare,
      prepare_amount: 0, // TODO set min amount
      frames: vec![Frame::StreamMoney(StreamMoneyFrame {
        stream_id: BigUint::from(stream_id),
        shares: BigUint::from(amount),
      })],
    };

    self.send_packet(amount, stream_packet)
  }

  fn send_packet(
    &mut self,
    amount: u64,
    packet: StreamPacket,
  ) -> impl Future<Item = (), Error = ()> {
    let encrypted = packet.to_encrypted(self.shared_secret.clone()).unwrap();
    let condition = generate_condition(self.shared_secret.clone(), encrypted.clone());
    let prepare = IlpPacket::Prepare(IlpPrepare::new(
      self.destination_account.to_string(),
      amount,
      condition,
      Utc::now() + Duration::seconds(30),
      encrypted,
    ));
    let request_id = self.next_request_id.fetch_add(1, Ordering::SeqCst) as u32;
    let request = (request_id, prepare);
    debug!(
      "Sending STREAM packet: {:?} in ILP packet: {:?}",
      packet, request
    );
    let wait_for_response = self.wait_for_response(request_id);
    self.outgoing.clone().send(request)
      .map(|_| ())
      .map_err(move |err| {
        error!("Error sending outgoing packet with request id {}: {:?} {:?}", request_id.clone(), err, packet);
      })
      .and_then(|_| wait_for_response)
      .and_then(move |response| {
        if let IlpPacket::Fulfill(fulfill) = response {
          debug!("Request {} was fulfilled", request_id.clone());
          Ok(())
        } else if let IlpPacket::Reject(reject) = response {
          error!("Request {} was rejected: {:?}", request_id.clone(), reject);
          Err(())
        } else {
          Err(())
        }
      })
  }

  fn wait_for_response(&mut self, request_id: u32) -> impl Future<Item = IlpPacket, Error = ()> {
    WaitForResponse {
      request_id,
      incoming: Arc::clone(&self.incoming),
      incoming_queue: self.incoming_queue.clone(),
    }
  }

  pub fn create_stream(&mut self) -> DataMoneyStream {
    let id = self.next_stream_id.fetch_add(2, Ordering::SeqCst) as u64;

    debug!("Created stream {}", id);

    DataMoneyStream {
      id,
      conn: self.clone(),
    }
  }

  fn handle_packet(&self, request: IlpRequest) -> impl Future<Item = (), Error = ()> {
    debug!("Handling packet: {:?}", request);
    ok(())
  }

  fn send_handshake(&self) {
    self.send_unfulfillable_prepare(StreamPacket {
      sequence: 0,
      ilp_packet_type: PacketType::IlpPrepare,
      prepare_amount: 0,
      frames: vec![Frame::ConnectionNewAddress(ConnectionNewAddressFrame {
        source_account: self.source_account.to_string(),
      })],
    });
  }

  fn send_unfulfillable_prepare(&self, stream_packet: StreamPacket) -> () {
    let request_id = 1;
    let prepare = IlpPacket::Prepare(IlpPrepare::new(
      // TODO do we need to clone this?
      self.destination_account.to_string(),
      0,
      random_condition(),
      Utc::now() + Duration::seconds(30),
      stream_packet
        .to_encrypted(self.shared_secret.clone())
        .unwrap(),
    ));
    self.outgoing.unbounded_send((request_id, prepare)).unwrap();
  }
}

struct WaitForResponse {
  request_id: u32,
  incoming: Arc<Mutex<UnboundedReceiver<IlpRequest>>>,
  incoming_queue: UnboundedSender<IlpRequest>,
}

impl Future for WaitForResponse {
  type Item = IlpPacket;
  type Error = ();

  fn poll(&mut self) -> Poll<Self::Item, Self::Error> {
    if let Ok(ref mut incoming) = self.incoming.try_lock() {
      let item = try_ready!(incoming.poll().map_err(|err| {
        error!("Error polling incoming {:?}", err);
      }));
      if let Some((request_id, packet)) = item {
        // TODO check that the response is valid
        if request_id == self.request_id {
          Ok(Async::Ready(packet))
        } else {
          self
            .incoming_queue
            .clone() // TODO avoid cloning this?
            .send((request_id, packet))
            .map_err(|err| {
              error!("Error re-queuing request {} {:?}", request_id, err);
            })
            // TODO handle this in an async way (buffering item if it doesn't send)
            .wait().unwrap();
          trace!("Requeued response to request {}", request_id);
          Ok(Async::NotReady)
        }
      } else {
        error!(
          "Stream ended before we got response for request ID: {}",
          self.request_id
        );
        Err(())
      }
    } else {
      Ok(Async::NotReady)
    }
  }
}

// fn parse_stream_packet_from_request(shared_secret: Bytes, (_id: u32, packet: IlpPacket)) -> (Option<StreamPacket>, bool) {
//     match packet {
//       IlpPacket::Prepare(packet) => {
//         // Check that the condition matches what we regenerate
//         let fulfillment = generate_fulfillment(&self.shared_secret[..], &packet.data[..]);
//         let condition = fulfillment_to_condition(&fulfillment);
//         let fulfillable = condition == packet.execution_condition;

//         // Check if we can decrypt and parse the STREAM packet
//         // TODO don't copy the packet data
//         let stream_packet = StreamPacket::from_encrypted(shared_secret, BytesMut::from(&packet.data[..]))
//           .map_err(|err| {
//             warn!("Got ILP packet with data we cannot parse: {:?}", packet);
//           })
//           .ok();
//         (stream_packet, fulfillable)
//       }
//       IlpPacket::Fulfill(packet) => {
//         // Check if we can decrypt and parse the STREAM packet
//         if let Ok(stream_packet) =
//           StreamPacket::from_encrypted(&self.shared_secret[..], &packet.data[..])
//         {
//           Ok(Async::Ready(Some((
//             request_id,
//             IlpPacket::Fulfill(packet),
//             Some(stream_packet),
//           ))))
//         } else {
//           warn!(
//             "Got ILP Fulfill for request: {} with no data attached: {:?}",
//             request_id, packet
//           );
//           Ok(Async::Ready(Some((
//             request_id,
//             IlpPacket::Fulfill(packet),
//             None,
//           ))))
//         }
//       }
//       IlpPacket::Reject(packet) => {
//         // Check if we can decrypt and parse the STREAM packet
//         if let Ok(stream_packet) =
//           StreamPacket::from_encrypted(&self.shared_secret[..], &packet.data[..])
//         {
//           Ok(Async::Ready(Some((
//             request_id,
//             IlpPacket::Reject(packet),
//             Some(stream_packet),
//           ))))
//         } else {
//           Ok(Async::Ready(Some((
//             request_id,
//             IlpPacket::Reject(packet),
//             None,
//           ))))
//         }
//       }
//       IlpPacket::Unknown => {
//         warn!("Got ILP packet with no data: {:?}", packet);
//         let reject = (
//           request_id,
//           IlpPacket::Reject(IlpReject::new("F06", "", "", Bytes::new())),
//         );
//         self.try_start_send(reject)?;
//         Ok(Async::NotReady)
//       }
//     }
// }
