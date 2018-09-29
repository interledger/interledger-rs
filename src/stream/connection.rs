use super::crypto::{generate_condition, random_condition, generate_fulfillment, fulfillment_to_condition};
use super::packet::*;
use super::StreamPacket;
use bytes::{Bytes, BytesMut};
use chrono::{Duration, Utc};
use futures::{Async, Poll, Stream};
use ilp::{IlpFulfill, IlpPacket, IlpPrepare, IlpReject, PacketType};
use num_bigint::BigUint;
use num_traits::ToPrimitive;
use plugin::IlpRequest;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use futures::sync::mpsc::{UnboundedSender, UnboundedReceiver};
use std::collections::{HashMap, VecDeque};
use hex;
use super::data_money_stream::{DataMoneyStream, DataMoneyStreamInternal, MoneyStreamInternal, DataStreamInternal};

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
  streams: Arc<RwLock<HashMap<u64, DataMoneyStream>>>,
  pending_outgoing_packets: Arc<Mutex<HashMap<u32, (u64, StreamPacket)>>>,
  new_streams: Arc<Mutex<VecDeque<u64>>>,
  frames_to_resend: Arc<Mutex<Vec<Frame>>>,
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
      frames_to_resend: Arc::new(Mutex::new(Vec::new())),
    };

    // TODO figure out a better way to send the initial packet - get the exchange rate and wait for response
    if !is_server {
      conn.send_handshake();
    }

    conn
  }

  pub fn create_stream(&self) -> DataMoneyStream {
    let id = self.next_stream_id.fetch_add(2, Ordering::SeqCst) as u64;
    let stream = DataMoneyStream::new(id, self.clone());
    self.streams.write().unwrap().insert(id, stream.clone());
    debug!("Created stream {}", id);
    stream
  }

  fn handle_incoming_prepare(&self, request_id: u32, prepare: IlpPrepare) -> Result<(), ()> {
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

    debug!("Prepare {} had stream packet: {:?}", request_id, stream_packet);

    // Handle new streams
    for frame in stream_packet.frames.iter() {
      match frame {
        Frame::StreamMoney(frame) => {
          self.handle_new_stream(frame.stream_id.to_u64().unwrap());
        },
        Frame::StreamData(frame) => {
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
          stream.money.add_received(amount);
        }
      }
    }

    self.handle_incoming_data(&stream_packet).unwrap();

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
    let stream = DataMoneyStream::new(stream_id, self.clone());
    if is_new {
      debug!("Got new stream {}", stream_id);
      self.streams.write().unwrap().insert(stream_id, stream);
      self.new_streams.lock().unwrap().push_back(stream_id);
    }
  }

  fn handle_incoming_data(&self, stream_packet: &StreamPacket) -> Result<(), ()> {
    for frame in stream_packet.frames.iter() {
      if let Frame::StreamData(frame) = frame {
        let stream_id = frame.stream_id.to_u64().unwrap();
        let streams = self.streams.read().unwrap();
        let stream = streams.get(&stream_id).unwrap();
        // TODO make sure the offset number isn't too big
        let data = frame.data.clone();
        let offset = frame.offset.to_usize().unwrap();
        debug!("Stream {} got {} bytes of incoming data", stream.id, data.len());
        stream.data.push_incoming_data(data, offset)?;
      }
    }
    Ok(())
  }

  fn handle_fulfill(&self, request_id: u32, fulfill: IlpFulfill) -> Result<(), ()> {
    debug!("Request {} was fulfilled with fulfillment: {}", request_id, hex::encode(&fulfill.fulfillment[..]));

    let (original_amount, original_packet) = self.pending_outgoing_packets.lock().unwrap().remove(&request_id).unwrap();

    let response = {
      let decrypted = StreamPacket::from_encrypted(self.shared_secret.clone(), BytesMut::from(fulfill.data)).ok();
      if let Some(packet) = decrypted {
        if packet.sequence != original_packet.sequence {
          warn!("Got Fulfill with stream packet whose sequence does not match the original request. Request ID: {}, sequence: {}, fulfill packet: {:?}", request_id, original_packet.sequence, packet);
          None
        } else if packet.ilp_packet_type != PacketType::IlpFulfill {
          warn!("Got Fulfill with stream packet that should have been on a differen type of ILP packet. Request ID: {}, fulfill packet: {:?}", request_id, packet);
          None
        } else {
          trace!("Got Fulfill with stream packet: {:?}", packet);
          Some(packet)
        }
      } else {
        None
      }
    };

    let total_delivered = {
      match response.as_ref() {
        Some(packet) => packet.prepare_amount,
        None => 0
      }
    };

    for frame in original_packet.frames.iter() {
      match frame {
        Frame::StreamMoney(frame) => {
          let stream_id = frame.stream_id.to_u64().unwrap();
          let streams = self.streams.read().unwrap();
          let stream = streams.get(&stream_id).unwrap();

          let shares = frame.shares.to_u64().unwrap();
          stream.money.pending_to_sent(shares);

          let amount_delivered: u64 = total_delivered * shares / original_amount;
          stream.money.add_delivered(amount_delivered);
        },
        _ => {},
      }
    }

    if let Some(packet) = response.as_ref() {
      self.handle_incoming_data(&packet)?;
    }

    // TODO handle response frames

    Ok(())
  }

  fn handle_reject(&self, request_id: u32, reject: IlpReject) -> Result<(), ()> {
    debug!("Request {} was rejected with code: {}", request_id, reject.code);

    let entry = self.pending_outgoing_packets.lock().unwrap().remove(&request_id);
    if entry.is_none() {
      return Ok(());
    }
    let (_original_amount, mut original_packet) = entry.unwrap();

    let response = {
      let decrypted = StreamPacket::from_encrypted(self.shared_secret.clone(), BytesMut::from(reject.data)).ok();
      if let Some(packet) = decrypted {
        if packet.sequence != original_packet.sequence {
          warn!("Got Reject with stream packet whose sequence does not match the original request. Request ID: {}, sequence: {}, packet: {:?}", request_id, original_packet.sequence, packet);
          None
        } else if packet.ilp_packet_type != PacketType::IlpReject {
          warn!("Got Reject with stream packet that should have been on a differen type of ILP packet. Request ID: {}, packet: {:?}", request_id, packet);
          None
        } else {
          trace!("Got Reject with stream packet: {:?}", packet);
          Some(packet)
        }
      } else {
        None
      }
    };

    let streams = self.streams.read().unwrap();

    // Release pending money
    for frame in original_packet.frames.iter() {
      match frame {
        Frame::StreamMoney(frame) => {
          let stream_id = frame.stream_id.to_u64().unwrap();
          let stream = streams.get(&stream_id).unwrap();

          let shares = frame.shares.to_u64().unwrap();
          stream.money.subtract_from_pending(shares);
        },
        _ => {},
      }
    }
    // TODO handle response frames

    if let Some(packet) = response.as_ref() {
      self.handle_incoming_data(&packet)?;
    }

    // Only resend frames if they didn't get to the receiver
    if response.is_none() {
      let mut frames_to_resend = self.frames_to_resend.lock().unwrap();
      while original_packet.frames.len() > 0 {
        match original_packet.frames.pop().unwrap() {
          Frame::StreamData(frame) => frames_to_resend.push(Frame::StreamData(frame)),
          _ => {}
        }
      }
    }

    Ok(())
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

}

impl Stream for Connection {
  type Item = DataMoneyStream;
  type Error = ();

  fn poll(&mut self) -> Poll<Option<Self::Item>, Self::Error> {
    trace!("Polling for new incoming streams");
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

// Only used by other modules in this crate
pub trait ConnectionInternal {
  fn try_send(&self) -> Result<(), ()>;
  fn try_handle_incoming(&self) -> Result<(), ()>;
}

impl ConnectionInternal for Connection {
  fn try_send(&self) -> Result<(), ()> {
    trace!("Checking if we should send an outgoing packet");

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
      trace!("Checking if stream {} has money or data to send", stream.id);
      let amount_to_send = stream.money.send_max() - stream.money.pending() - stream.money.total_sent();
      if amount_to_send > 0 {
        trace!("Stream {} sending {}", stream.id, amount_to_send);
        stream.money.add_to_pending(amount_to_send);
        outgoing_amount += amount_to_send;
        frames.push(Frame::StreamMoney(StreamMoneyFrame {
          stream_id: BigUint::from(stream.id),
          shares: BigUint::from(amount_to_send),
        }));
      } else {
        debug!("Stream {} does not have any money to send", stream.id);
      }

      // Send data
      // TODO don't send too much data
      let max_data: usize = 1000000000;
      if let Some((data, offset)) = stream.data.get_outgoing_data(max_data) {
        trace!("Stream {} has {} bytes to send (offset: {})", stream.id, data.len(), offset);
        frames.push(Frame::StreamData(StreamDataFrame {
          stream_id: BigUint::from(stream.id),
          data,
          offset: BigUint::from(offset),
        }))
      } else {
        trace!("Stream {} does not have any data to send", stream.id);
      }
    }

    if frames.len() == 0 {
      trace!("Not sending packet, no frames need to be sent");
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
    })?.insert(request_id, (outgoing_amount, stream_packet.clone()));

    self.outgoing.unbounded_send(request)
      .map_err(|err| {
        error!("Error sending outgoing packet: {:?}", err);
      })?;

    Ok(())
  }

  fn try_handle_incoming(&self) -> Result<(), ()> {
    // Handle incoming requests until there are no more
    // Note: looping until we get Async::NotReady tells Tokio to wake us up when there are more incoming requests
    loop {
      trace!("Polling for incoming requests");
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
          trace!("No more incoming requests for now");
          return Ok(())
        },
        Err(err) => {
          error!("Error polling incoming request stream: {:?}", err);
          return Err(())
        }
      };
    }
  }
}