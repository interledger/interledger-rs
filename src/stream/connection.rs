use super::{StreamPacketStream, StreamRequest};
use super::packet::*;
use futures::{Future, Stream, Sink};
use futures::sync::mpsc::{unbounded, SendError, UnboundedSender, UnboundedReceiver};
use futures::future::ok;
use tokio::spawn;
use ilp::{IlpPacket, IlpPrepare, IlpFulfill};
use bytes::Bytes;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use plugin::{Plugin, IlpRequest};

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
  plugin_sender: UnboundedSender<StreamRequest>,
  source_account: String,
  destination_account: String,
  streams: Arc<Mutex<HashMap<u64, DataMoneyStreamInternal>>>,
  next_stream_id: u64,
}

impl Connection {
  pub fn new<S>(
    plugin: StreamPacketStream<S>,
    source_account: String,
    destination_account: String,
    is_server: bool,
  ) -> Self
  where
    S: Plugin<Item = IlpRequest, Error = (), SinkItem = IlpRequest, SinkError = ()>,
  {
    let next_stream_id = match is_server {
      true => 2,
      false => 1,
    };

    let (sender, receiver) = unbounded::<StreamRequest>();
    let plugin = Arc::new(plugin);

    let forward_to_plugin = Arc::clone(&mut plugin)
      .send_all(receiver)
      .map(|_| ())
      .map_err(|err| {
        error!("Error forwarding request to plugin: {:?}", err);
      });
    spawn(forward_to_plugin);

    let connection = Connection {
      plugin_sender: sender,
      source_account,
      destination_account,
      streams: Arc::new(Mutex::new(HashMap::new())),
      next_stream_id,
    };

    let handle_packets = Arc::clone(&mut plugin)
      .for_each(|(request_id, packet, stream_packet)| {
        connection.handle_packet(request_id, packet, stream_packet)
      }).map(|_| ())
      .map_err(|err| {
        error!("Error handling incoming packet: {:?}", err);
      });
    spawn(handle_packets);

    connection
  }

  pub fn create_stream(&mut self) -> DataMoneyStream {
    let id = self.next_stream_id;
    self.next_stream_id += 1;

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

    self.streams.lock().unwrap().insert(id, internal);

    DataMoneyStream {
      id,
      incoming_data: recv_incoming_data,
      outgoing_data: send_outgoing_data,
      incoming_money: recv_incoming_money,
      outgoing_money: send_outgoing_money,
    }
  }

  fn handle_packet(&self, request_id: u32, packet: IlpPacket, stream_packet: Option<StreamPacket>) -> impl Future<Item = (), Error = ()> {
    println!("STREAM got packet {:?} {:?}", packet, stream_packet);

    ok(())
  }
}

// impl Stream for Connection<S> {
//   type Item = DataMoneyStream;
//   type Error = ();

//   fn poll(&mut self) -> Poll<Option<Self::Item>, Self::Error> {}
// }
