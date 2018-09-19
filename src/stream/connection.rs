use super::packet::*;
use super::{StreamPacket, StreamRequest};
use bytes::Bytes;
use futures::future::ok;
use futures::sync::mpsc::{unbounded, SendError, UnboundedReceiver, UnboundedSender};
use futures::{Future, Sink, Stream};
use ilp::{IlpFulfill, IlpPacket, IlpPrepare, PacketType};
use plugin::{IlpRequest, Plugin};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, RwLock, RwLockReadGuard, RwLockWriteGuard};
use tokio;
use super::crypto::{random_condition};
use chrono::{Utc, Duration};

pub struct DataMoneyStream {
  pub id: u64,
  incoming_data: UnboundedReceiver<Bytes>,
  outgoing_data: UnboundedSender<Bytes>,
  incoming_money: UnboundedReceiver<u64>,
  outgoing_money: UnboundedSender<u64>,
}

// Used on the connection side
struct DataMoneyStreamInternal {
  // These are named the same as for the DataMoneyStream but they are the opposite sides of the channels
  outgoing_data: UnboundedReceiver<Bytes>,
  incoming_data: UnboundedSender<Bytes>,
  outgoing_money: UnboundedReceiver<u64>,
  incoming_money: UnboundedSender<u64>,
}

// TODO can we hardcode the type of S to be a Plugin?
pub struct Connection {
  outgoing: UnboundedSender<IlpRequest>,
  shared_secret: Bytes,
  source_account: String,
  destination_account: String,
  streams: Arc<RwLock<HashMap<u64, DataMoneyStreamInternal>>>,
  next_stream_id: u64,
}

impl Connection {
  pub fn new(
    outgoing: UnboundedSender<IlpRequest>,
    incoming: UnboundedReceiver<IlpRequest>,
    shared_secret: Bytes,
    source_account: String,
    destination_account: String,
    is_server: bool,
  ) -> Arc<Self> {
    let next_stream_id = match is_server {
      true => 2,
      false => 1,
    };

    let conn = Arc::new(Connection {
      outgoing,
      shared_secret,
      source_account,
      destination_account,
      streams: Arc::new(RwLock::new(HashMap::new())),
      next_stream_id,
    });

    // TODO how do we stop it from handling more packets if we want to close the connection?
    let conn_clone = Arc::clone(&conn);
    let handle_packets = incoming.for_each(move |request| conn_clone.handle_packet(request));
    tokio::spawn(handle_packets);

    conn.send_handshake();

    conn
  }

  pub fn create_stream(&mut self) -> DataMoneyStream {
    let id = self.next_stream_id;
    self.next_stream_id += 2;

    // TODO should we use bounded streams?
    let (send_incoming_data, recv_incoming_data) = unbounded::<Bytes>();
    let (send_outgoing_data, recv_outgoing_data) = unbounded::<Bytes>();
    let (send_incoming_money, recv_incoming_money) = unbounded::<u64>();
    let (send_outgoing_money, recv_outgoing_money) = unbounded::<u64>();

    let internal = DataMoneyStreamInternal {
      incoming_data: send_incoming_data,
      outgoing_data: recv_outgoing_data,
      incoming_money: send_incoming_money,
      outgoing_money: recv_outgoing_money,
    };

    self.streams.write().unwrap().insert(id, internal);

    DataMoneyStream {
      id,
      incoming_data: recv_incoming_data,
      outgoing_data: send_outgoing_data,
      incoming_money: recv_incoming_money,
      outgoing_money: send_outgoing_money,
    }
  }

  fn handle_packet(&self, request: IlpRequest) -> impl Future<Item = (), Error = ()> {
    println!("Got packet: {:?}", request);
    ok(())
  }

  fn send_handshake(&self) {
    self.send_unfulfillable_prepare(StreamPacket {
      sequence: 0,
      ilp_packet_type: PacketType::IlpPrepare,
      prepare_amount: 0,
      frames: vec![Frame::ConnectionNewAddress(ConnectionNewAddressFrame {
        source_account: self.source_account.clone()
      })]
    });
  }

  fn send_unfulfillable_prepare(&self, stream_packet: StreamPacket) -> () {
    let request_id = 1;
    let prepare = IlpPacket::Prepare(IlpPrepare::new(
      // TODO do we need to clone this?
      self.destination_account.clone(),
      0,
      random_condition(),
      Utc::now() + Duration::seconds(30),
      stream_packet.to_encrypted(self.shared_secret.clone()).unwrap()
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
