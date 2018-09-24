use super::crypto::{generate_condition, random_condition, generate_fulfillment, fulfillment_to_condition};
use super::packet::*;
use super::StreamPacket;
use bytes::{Bytes, BytesMut};
use chrono::{Duration, Utc};
use futures::{Async, Poll, Sink, Stream, StartSend, AsyncSink};
use ilp::{IlpFulfill, IlpPacket, IlpPrepare, IlpReject, PacketType};
use num_bigint::BigUint;
use num_traits::ToPrimitive;
use plugin::IlpRequest;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use futures::sync::mpsc::{UnboundedSender, UnboundedReceiver};
use std::collections::{HashMap, VecDeque};
use hex;

#[derive(Clone)]
pub struct MoneyStream {
  id: u64,
  // TODO should this be a reference?
  conn: Connection,
  send_max: Arc<AtomicUsize>,
  pending: Arc<AtomicUsize>,
  sent: Arc<AtomicUsize>,
  delivered: Arc<AtomicUsize>,
  received: Arc<AtomicUsize>,
  last_reported_received: Arc<AtomicUsize>,
}

impl MoneyStream {
  fn new(id: u64, connection: Connection) -> Self {
    MoneyStream {
      id,
      conn: connection,
      send_max: Arc::new(AtomicUsize::new(0)),
      pending: Arc::new(AtomicUsize::new(0)),
      sent: Arc::new(AtomicUsize::new(0)),
      delivered: Arc::new(AtomicUsize::new(0)),
      received: Arc::new(AtomicUsize::new(0)),
      last_reported_received: Arc::new(AtomicUsize::new(0)),
    }
  }

  pub fn total_sent(&self) -> u64 {
    self.sent.load(Ordering::SeqCst) as u64
  }

  pub fn total_delivered(&self) -> u64 {
    self.delivered.load(Ordering::SeqCst) as u64
  }

  pub fn total_received(&self) -> u64 {
    self.received.load(Ordering::SeqCst) as u64
  }

  fn pending(&self) -> u64 {
    self.pending.load(Ordering::SeqCst) as u64
  }

  fn add_to_pending(&self, amount: u64) {
    self.pending.fetch_add(amount as usize, Ordering::SeqCst);
  }

  fn pending_to_sent(&self, amount: u64) {
    self.pending.fetch_sub(amount as usize, Ordering::SeqCst);
    self.sent.fetch_add(amount as usize, Ordering::SeqCst);
  }

  fn send_max(&self) -> u64 {
    self.send_max.load(Ordering::SeqCst) as u64
  }

  fn add_received(&self, amount: u64) {
    self.received.fetch_add(amount as usize, Ordering::SeqCst);
  }

  // pub fn close(&mut self) -> impl Future<Item = (), Error = ()> {
  //   self.conn.close_stream(self.id)
  // }
}

impl Stream for MoneyStream {
  type Item = u64;
  type Error = ();

  fn poll(&mut self) -> Poll<Option<Self::Item>, Self::Error> {
    self.conn.try_handle_incoming()?;

    let total_received = self.received.load(Ordering::SeqCst);
    let last_reported_received = self.last_reported_received.load(Ordering::SeqCst);
    let amount_received = total_received - last_reported_received;
    if amount_received > 0 {
      self.last_reported_received.store(total_received, Ordering::SeqCst);
      Ok(Async::Ready(Some(amount_received as u64)))
    } else {
      Ok(Async::NotReady)
    }
  }
}

impl Sink for MoneyStream {
  type SinkItem = u64;
  type SinkError = ();

  fn start_send(&mut self, amount: u64) -> StartSend<Self::SinkItem, Self::SinkError> {
    self.send_max.fetch_add(amount as usize, Ordering::SeqCst);
    self.conn.try_send()?;
    Ok(AsyncSink::Ready)
  }

  fn poll_complete(&mut self) -> Poll<(), Self::SinkError> {
    self.conn.try_send()?;
    self.conn.try_handle_incoming()?;

    if self.sent.load(Ordering::SeqCst) >= self.send_max.load(Ordering::SeqCst) {
      Ok(Async::Ready(()))
    } else {
      Ok(Async::NotReady)
    }
  }
}

#[derive(Clone)]
pub struct Connection {
  state: Arc<RwLock<ConnectionState>>,
  // TODO should this be a bounded sender? Don't want too many outgoing packets in the queue
  outgoing: UnboundedSender<IlpRequest>,
  incoming: Arc<Mutex<UnboundedReceiver<IlpRequest>>>,
  shared_secret: Bytes,
  source_account: Arc<String>,
  destination_account: Arc<String>,
  next_stream_id: Arc<AtomicUsize>,
  next_packet_sequence: Arc<AtomicUsize>,
  next_request_id: Arc<AtomicUsize>,
  streams: Arc<RwLock<HashMap<u64, MoneyStream>>>,
  pending_outgoing_packets: Arc<Mutex<HashMap<u32, StreamPacket>>>,
  new_streams: Arc<Mutex<VecDeque<u64>>>,
  // TODO add connection-level stats
}

#[derive(PartialEq, Debug)]
enum ConnectionState {
  Opening,
  // Open,
  // Closed,
  // Closing,
}

impl Connection {
  pub fn new(
    outgoing: UnboundedSender<IlpRequest>,
    incoming: UnboundedReceiver<IlpRequest>,
    shared_secret: Bytes,
    source_account: String,
    destination_account: String,
    is_server: bool,
    next_request_id: Arc<AtomicUsize>,
  ) -> Self {
    let next_stream_id = match is_server {
      true => 2,
      false => 1,
    };

    let conn = Connection {
      state: Arc::new(RwLock::new(ConnectionState::Opening)),
      outgoing,
      incoming: Arc::new(Mutex::new(incoming)),
      shared_secret,
      source_account: Arc::new(source_account),
      destination_account: Arc::new(destination_account),
      next_stream_id: Arc::new(AtomicUsize::new(next_stream_id)),
      next_packet_sequence: Arc::new(AtomicUsize::new(1)),
      next_request_id,
      streams: Arc::new(RwLock::new(HashMap::new())),
      pending_outgoing_packets: Arc::new(Mutex::new(HashMap::new())),
      new_streams: Arc::new(Mutex::new(VecDeque::new())),
    };

    // TODO figure out a better way to send the initial packet - get the exchange rate and wait for response
    if !is_server {
      conn.send_handshake();
    }

    conn
  }

  fn try_send(&mut self) -> Result<(), ()> {
    debug!("Checking if we should send an outgoing packet");

    let mut outgoing_amount: u64 = 0;
    let mut frames: Vec<Frame> = Vec::new();

    let streams = {
      if let Ok(streams) = self.streams.read() {
        streams
      } else {
        debug!("Unable to get read lock on streams while trying to send");
        return Ok(());
      }
    };

    // TODO don't send more than max packet amount
    for stream in streams.values() {
      let amount_to_send = stream.send_max() - stream.pending() - stream.total_sent();
      if amount_to_send > 0 {
        stream.add_to_pending(amount_to_send);
        outgoing_amount += amount_to_send;
        frames.push(Frame::StreamMoney(StreamMoneyFrame {
          stream_id: BigUint::from(stream.id),
          shares: BigUint::from(amount_to_send),
        }));
      }
    }

    if frames.len() == 0 {
      debug!("Not sending packet, no frames need to be sent");
      return Ok(())
    }

    let stream_packet = StreamPacket {
      sequence: self.next_packet_sequence.fetch_add(1, Ordering::SeqCst) as u64,
      ilp_packet_type: PacketType::IlpPrepare,
      prepare_amount: 0, // TODO set min amount
      frames,
    };

    let encrypted = stream_packet.to_encrypted(self.shared_secret.clone()).unwrap();
    let condition = generate_condition(self.shared_secret.clone(), encrypted.clone());
    let prepare = IlpPrepare::new(
      self.destination_account.to_string(),
      outgoing_amount,
      condition,
      Utc::now() + Duration::seconds(30),
      encrypted,
    );
    let request_id = self.next_request_id.fetch_add(1, Ordering::SeqCst) as u32;
    let request = (request_id, IlpPacket::Prepare(prepare));
    debug!(
      "Sending outgoing request {} with stream packet: {:?}",
      request_id, stream_packet
    );

    self.pending_outgoing_packets.lock().map_err(|err| {
      error!("Cannot acquire lock on pending_outgoing_packets: {:?}", err);
    })?.insert(request_id, stream_packet.clone());

    self.outgoing.unbounded_send(request)
      .map_err(|err| {
        error!("Error sending outgoing packet: {:?}", err);
      })?;

    Ok(())
  }

  // TODO will Tokio register the interest even if we're not returning Async::NotReady?
  fn try_handle_incoming(&mut self) -> Result<(), ()> {
    // Handle incoming requests until there are no more
    // Note: looping until we get Async::NotReady tells Tokio to wake us up when there are more incoming requests
    loop {
      debug!("Checking for incoming request");
      let next = {
        if let Ok(mut incoming) = self.incoming.try_lock() {
          incoming.poll()
        } else {
          debug!("Unable to acquire lock on incoming stream");
          return Ok(());
        }
      };

      match next {
        Ok(Async::Ready(Some((request_id, packet)))) => {
          match packet {
            IlpPacket::Prepare(prepare) => self.handle_incoming_prepare(request_id, prepare)?,
            IlpPacket::Fulfill(fulfill) => self.handle_fulfill(request_id, fulfill)?,
            IlpPacket::Reject(reject) => self.handle_reject(request_id, reject)?,
            _ => {}
          }
        },
        Ok(Async::Ready(None)) => {
          error!("Incoming stream closed");
          // TODO should this error?
          return Ok(())
        },
        Ok(Async::NotReady) => {
          return Ok(())
        },
        Err(err) => {
          error!("Error polling incoming request stream: {:?}", err);
          return Err(())
        }
      };
    }
  }

  fn handle_incoming_prepare(&mut self, request_id: u32, prepare: IlpPrepare) -> Result<(), ()> {
    debug!("Handling incoming prepare {}", request_id);

    let response_frames: Vec<Frame> = Vec::new();

    let fulfillment = generate_fulfillment(self.shared_secret.clone(), prepare.data.clone());
    let condition = fulfillment_to_condition(fulfillment.clone());
    let is_fulfillable = condition == prepare.execution_condition;

    // TODO avoid copying data
    let stream_packet = StreamPacket::from_encrypted(self.shared_secret.clone(), BytesMut::from(prepare.data));
    if stream_packet.is_err() {
        warn!("Got Prepare with data that we cannot parse. Rejecting request {}", request_id);
        self.outgoing.unbounded_send((request_id, IlpPacket::Reject(IlpReject::new("F02", "", "", Bytes::new())))).map_err(|err| {
          error!("Error sending Reject {} {:?}", request_id, err);
        })?;
        return Ok(());
      }
    let stream_packet = stream_packet.unwrap();

    // Handle new streams
    for frame in stream_packet.frames.iter() {
      match frame {
        Frame::StreamMoney(frame) => {
          self.handle_new_stream(frame.stream_id.to_u64().unwrap());
        },
        // TODO handle other frames that open streams
        _ => {}
      }
    }

    // Count up the total number of money "shares" in the packet
    let total_money_shares: u64 = stream_packet.frames.iter().fold(0, |sum, frame| {
      if let Frame::StreamMoney(frame) = frame {
        sum + frame.shares.to_u64().unwrap()
      } else {
        sum
      }
    });

    // Handle incoming money
    if is_fulfillable {
      for frame in stream_packet.frames.iter() {
        if let Frame::StreamMoney(frame) = frame {
          // TODO only add money to incoming if sending the fulfill is successful
          // TODO make sure all other checks pass first
          let stream_id = frame.stream_id.to_u64().unwrap();
          let streams = self.streams.read().unwrap();
          let stream = streams.get(&stream_id).unwrap();
          let amount: u64 = frame.shares.to_u64().unwrap() * prepare.amount / total_money_shares;
          debug!("Stream {} received {}", stream_id, amount);
          stream.add_received(amount);
        }
      }
    }

    // Fulfill or reject Preapre
    if is_fulfillable {
      let response_packet = StreamPacket {
        sequence: stream_packet.sequence,
        ilp_packet_type: PacketType::IlpFulfill,
        prepare_amount: prepare.amount,
        frames: response_frames,
      };
      let encrypted_response = response_packet.to_encrypted(self.shared_secret.clone()).unwrap();
      let fulfill = IlpPacket::Fulfill(IlpFulfill::new(fulfillment.clone(), encrypted_response));
      debug!("Fulfilling request {} with fulfillment: {} and encrypted stream packet: {:?}", request_id, hex::encode(&fulfillment[..]), response_packet);
      self.outgoing.unbounded_send((request_id, fulfill)).unwrap();
    } else {
      let response_packet = StreamPacket {
        sequence: stream_packet.sequence,
        ilp_packet_type: PacketType::IlpReject,
        prepare_amount: prepare.amount,
        frames: response_frames,
      };
      let encrypted_response = response_packet.to_encrypted(self.shared_secret.clone()).unwrap();
      let reject = IlpPacket::Reject(IlpReject::new("F99", "", "", encrypted_response));
      debug!("Rejecting request {} and including encrypted stream packet {:?}", request_id, response_packet);
      self.outgoing.unbounded_send((request_id, reject)).unwrap();
    }

    Ok(())
  }

  fn handle_new_stream(&self, stream_id: u64) {
    // TODO make sure they don't open streams with our number (even or odd, depending on whether we're the client or server)
    let is_new = !self.streams.read().unwrap().contains_key(&stream_id);
    let stream = MoneyStream::new(stream_id, self.clone());
    if is_new {
      self.streams.write().unwrap().insert(stream_id, stream);
    }
    debug!("Got new stream {}", stream_id);
  }

  fn handle_fulfill(&mut self, request_id: u32, fulfill: IlpFulfill) -> Result<(), ()> {
    debug!("Request {} was fulfilled with fulfillment: {}", request_id, hex::encode(&fulfill.fulfillment[..]));

    let original_request = self.pending_outgoing_packets.lock().unwrap().remove(&request_id).unwrap();

    for frame in original_request.frames.iter() {
      match frame {
        Frame::StreamMoney(frame) => {
          let stream_id = frame.stream_id.to_u64().unwrap();
          let streams = self.streams.read().unwrap();
          let stream = streams.get(&stream_id).unwrap();
          let amount = frame.shares.to_u64().unwrap();
          stream.pending_to_sent(amount);
        },
        _ => {},
      }
    }

    // TODO handle response frames

    Ok(())
  }

  fn handle_reject(&mut self, request_id: u32, reject: IlpReject) -> Result<(), ()> {
    debug!("Request {} was rejected with code: {}", request_id, reject.code);

    // TODO handle response frames

    Ok(())
  }

  // pub fn close(&mut self) -> impl Future<Item = (), Error = ()> {
  //   {
  //     let mut state = self.state.write().unwrap();
  //     *state = ConnectionState::Closing;
  //   }

  //   let stream_packet = StreamPacket {
  //     sequence: self.next_packet_sequence.fetch_add(1, Ordering::SeqCst) as u64,
  //     ilp_packet_type: PacketType::IlpPrepare,
  //     prepare_amount: 0,
  //     frames: vec![Frame::ConnectionClose(ConnectionCloseFrame {
  //       code: ErrorCode::NoError,
  //       message: String::new(),
  //     })],
  //   };

  //   let state = Arc::clone(&self.state);
  //   self.send_packet(0, stream_packet).and_then(move |_| {
  //     let mut state = state.write().unwrap();
  //     *state = ConnectionState::Closed;
  //     Ok(())
  //   })
  // }

  pub fn create_stream(&mut self) -> MoneyStream {
    let id = self.next_stream_id.fetch_add(2, Ordering::SeqCst) as u64;
    debug!("Created stream {}", id);

    let stream = MoneyStream::new(id, self.clone());

    self.streams.write().unwrap().insert(id, stream.clone());

    stream
  }

  fn send_handshake(&self) {
    let sequence = self.next_packet_sequence.fetch_add(1, Ordering::SeqCst) as u64;
    self.send_unfulfillable_prepare(StreamPacket {
      sequence,
      ilp_packet_type: PacketType::IlpPrepare,
      prepare_amount: 0,
      frames: vec![Frame::ConnectionNewAddress(ConnectionNewAddressFrame {
        source_account: self.source_account.to_string(),
      })],
    });
  }

  // TODO wait for response
  fn send_unfulfillable_prepare(&self, stream_packet: StreamPacket) -> () {
    let request_id = self.next_request_id.fetch_add(1, Ordering::SeqCst) as u32;
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

impl Stream for Connection {
  type Item = MoneyStream;
  type Error = ();

  fn poll(&mut self) -> Poll<Option<Self::Item>, Self::Error> {
    self.try_handle_incoming()?;

    if let Ok(mut new_streams) = self.new_streams.try_lock() {
      if let Some(stream_id) = new_streams.pop_front() {
        if let Ok(streams) = self.streams.read() {
          Ok(Async::Ready(Some(streams.get(&stream_id).unwrap().clone())))
        } else {
          debug!("Unable to acquire lock on streams while polling for new streams");
          new_streams.push_back(stream_id);
          Ok(Async::NotReady)
        }
      } else {
        Ok(Async::NotReady)
      }
    } else {
      debug!("Unable to acquire lock on new_streams");
      // TODO should we return an error here?
      Ok(Async::NotReady)
    }
  }
}
