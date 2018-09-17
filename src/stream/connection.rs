use super::{StreamPacketStream};
use futures::{Stream, Sink};
use futures::sync::mpsc::{unbounded, SendError};
use ilp::{IlpPacket, IlpPrepare, IlpFulfill};
use bytes::Bytes;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use plugin::{Plugin, IlpRequest};

pub struct DataMoneyStream {
  pub id: u64,
  incoming_data: Arc<Mutex<Stream<Item = Bytes, Error = ()>>>,
  outgoing_data: Arc<Mutex<Sink<SinkItem = Bytes, SinkError = SendError<Bytes>>>>,
  incoming_money: Arc<Mutex<Stream<Item = u64, Error = ()>>>,
  outgoing_money: Arc<Mutex<Sink<SinkItem = u64, SinkError = SendError<u64>>>>,
}

// Used on the connection side
struct DataMoneyStreamInternal {
  outgoing_data: Arc<Mutex<Stream<Item = Bytes, Error = ()>>>,
  incoming_data: Arc<Mutex<Sink<SinkItem = Bytes, SinkError = SendError<Bytes>>>>,
  outgoing_money: Arc<Mutex<Stream<Item = u64, Error = ()>>>,
  incoming_money: Arc<Mutex<Sink<SinkItem = u64, SinkError = SendError<u64>>>>,
}

// TODO can we hardcode the type of S to be a Plugin?
pub struct Connection<S: Stream + Sink> {
  plugin: StreamPacketStream<S>,
  source_account: String,
  destination_account: String,
  streams: Arc<Mutex<HashMap<u64, DataMoneyStreamInternal>>>,
  next_stream_id: u64,
}

impl<S> Connection<S>
where
  S: Plugin<Item = IlpRequest, Error = (), SinkItem = IlpRequest, SinkError = ()> + Sized,
{
  pub fn new(
    plugin: StreamPacketStream<S>,
    source_account: String,
    destination_account: String,
    is_server: bool,
  ) -> Self
  {
    let next_stream_id = match is_server {
      true => 2,
      false => 1,
    };
    Connection {
      plugin,
      source_account,
      destination_account,
      streams: Arc::new(Mutex::new(HashMap::new())),
      next_stream_id,
    }
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
      incoming_data: Arc::new(Mutex::new(send_incoming_data)),
      outgoing_data: Arc::new(Mutex::new(recv_outgoing_data)),
      incoming_money: Arc::new(Mutex::new(send_incoming_money)),
      outgoing_money: Arc::new(Mutex::new(recv_outgoing_money)),
    };

    self.streams.lock().unwrap().insert(id, internal);

    DataMoneyStream {
      id,
      incoming_data: Arc::new(Mutex::new(recv_incoming_data)),
      outgoing_data: Arc::new(Mutex::new(send_outgoing_data)),
      incoming_money: Arc::new(Mutex::new(recv_incoming_money)),
      outgoing_money: Arc::new(Mutex::new(send_outgoing_money)),
    }
  }
}

// impl Stream for Connection<S> {
//   type Item = DataMoneyStream;
//   type Error = ();

//   fn poll(&mut self) -> Poll<Option<Self::Item>, Self::Error> {}
// }
