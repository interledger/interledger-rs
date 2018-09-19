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
  pub id: u64,
  pub incoming_data: UnboundedReceiver<Bytes>,
  pub outgoing_data: UnboundedSender<Bytes>,
  pub incoming_money: UnboundedReceiver<u64>,
  pub outgoing_money: UnboundedSender<u64>,
}

// Used on the connection side
struct DataMoneyStreamInternal {
  // These are named the same as for the DataMoneyStream but they are the opposite sides of the channels
  outgoing_data: Arc<Mutex<UnboundedReceiver<Bytes>>>,
  incoming_data: Arc<Mutex<UnboundedSender<Bytes>>>,
  outgoing_money: Arc<Mutex<UnboundedReceiver<u64>>>,
  incoming_money: Arc<Mutex<UnboundedSender<u64>>>,
  outgoing_data_offset: Arc<AtomicUsize>,
}

#[derive(Clone)]
pub struct Connection {
  state: Arc<RwLock<ConnectionState>>,
  outgoing: UnboundedSender<IlpRequest>,
  shared_secret: Bytes,
  source_account: Arc<String>,
  destination_account: Arc<String>,
  streams: Arc<RwLock<HashMap<u64, DataMoneyStreamInternal>>>,
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

    let conn = Connection {
      state: Arc::new(RwLock::new(ConnectionState::Opening)),
      outgoing,
      shared_secret,
      source_account: Arc::new(source_account),
      destination_account: Arc::new(destination_account),
      streams: Arc::new(RwLock::new(HashMap::new())),
      next_stream_id: Arc::new(AtomicUsize::new(next_stream_id)),
      next_packet_sequence: Arc::new(AtomicUsize::new(1)),
      next_request_id: Arc::new(AtomicUsize::new(1)),
    };

    // Handle incoming packets
    // TODO how do we stop it from handling more packets if we want to close the connection?
    let conn_clone = conn.clone();
    let handle_packets = incoming.for_each(move |request| conn_clone.handle_packet(request));
    tokio::spawn(handle_packets);

    conn.send_handshake();

    // Send outgoing packets
    let outgoing_conn = conn.clone();
    let send_outgoing = poll_fn(move || -> Poll<Option<(IlpRequest, Connection)>, ()> {
      debug!("Polling for outgoing packet");
      if *outgoing_conn.state.read().unwrap() == ConnectionState::Closed {
        return Ok(Async::Ready(None));
      }
      if let Some(request) = outgoing_conn.load_outgoing_packet() {
        Ok(Async::Ready(Some((request, outgoing_conn.to_owned()))))
      } else {
        // TODO is this going to cause problems because the function won't keep polling?
        Ok(Async::NotReady)
      }
    }).for_each(move |(request, conn)| {
      conn.outgoing.send(request).map(|_| ()).map_err(|err| {
        error!("Error sending outgoing request {:?}", err);
      })
    });
    tokio::spawn(send_outgoing);

    conn
  }

  pub fn create_stream(&mut self) -> DataMoneyStream {
    let id = self.next_stream_id.fetch_add(2, Ordering::SeqCst) as u64;

    // TODO should we use bounded streams?
    let (send_incoming_data, recv_incoming_data) = unbounded::<Bytes>();
    let (send_outgoing_data, recv_outgoing_data) = unbounded::<Bytes>();
    let (send_incoming_money, recv_incoming_money) = unbounded::<u64>();
    let (send_outgoing_money, recv_outgoing_money) = unbounded::<u64>();

    let internal = DataMoneyStreamInternal {
      incoming_data: Arc::new(Mutex::new(send_incoming_data)),
      outgoing_data: Arc::new(Mutex::new(recv_outgoing_data)),
      incoming_money: Arc::new(Mutex::new(send_incoming_money)),
      outgoing_money: Arc::new(Mutex::new(recv_outgoing_money)),
      outgoing_data_offset: Arc::new(AtomicUsize::new(0)),
    };

    self.streams.write().unwrap().insert(id, internal);

    debug!("Created stream {}", id);

    DataMoneyStream {
      id,
      incoming_data: recv_incoming_data,
      outgoing_data: send_outgoing_data,
      incoming_money: recv_incoming_money,
      outgoing_money: send_outgoing_money,
    }
  }

  fn handle_packet(&self, request: IlpRequest) -> impl Future<Item = (), Error = ()> {
    debug!("Handling packet: {:?}", request);
    ok(())
  }

  fn load_outgoing_packet(&self) -> Option<IlpRequest> {
    debug!("Loading outgoing packet");

    let mut frames: Vec<Frame> = Vec::new();
    let mut outgoing_amount: u64 = 0;

    let streams = self.streams.read().ok()?;
    for (stream_id, mut stream) in streams.iter() {
      if let Ok(ref mut outgoing_money) = stream.outgoing_money.try_lock() {
        if let Ok(Async::Ready(Some(amount))) = outgoing_money.poll() {
          outgoing_amount += amount;
          frames.push(Frame::StreamMoney(StreamMoneyFrame {
            stream_id: BigUint::from(*stream_id),
            shares: BigUint::from(amount),
          }));
          debug!("Stream {} is going to send {}", stream_id, amount);
        }
      } else {
        debug!(
          "Unable to get lock on outgoing_money for stream {}",
          stream_id
        );
      }

      if let Ok(ref mut outgoing_data) = stream.outgoing_data.try_lock() {
        if let Ok(Async::Ready(Some(data))) = outgoing_data.poll() {
          let offset = stream
            .outgoing_data_offset
            .fetch_add(data.len(), Ordering::SeqCst);
          frames.push(Frame::StreamData(StreamDataFrame {
            stream_id: BigUint::from(*stream_id),
            data: data.to_vec(),
            offset: BigUint::from(offset),
          }));
        }
      } else {
        debug!(
          "Unable to get lock on outgoing_data for stream {}",
          stream_id
        );
      }
    }

    if frames.len() == 0 {
      None
    } else {
      let stream_packet = StreamPacket {
        sequence: self.next_packet_sequence.fetch_add(1, Ordering::SeqCst) as u64,
        ilp_packet_type: PacketType::IlpPrepare,
        prepare_amount: 0,
        frames,
      };
      let encrypted = stream_packet
        .to_encrypted(self.shared_secret.clone())
        .unwrap();
      let condition = generate_condition(self.shared_secret.clone(), encrypted.clone());
      let packet = IlpPacket::Prepare(IlpPrepare::new(
        self.destination_account.to_string(),
        outgoing_amount,
        condition,
        Utc::now() + Duration::seconds(30),
        encrypted,
      ));
      let request_id = self.next_request_id.fetch_add(1, Ordering::SeqCst) as u32;
      let request = (request_id, packet);
      debug!(
        "Sending STREAM packet: {:?} in ILP packet: {:?}",
        stream_packet, request
      );
      Some(request)
    }
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
