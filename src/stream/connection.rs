use super::crypto::{generate_condition, random_condition};
use super::packet::*;
use super::StreamPacket;
use bytes::{Bytes, BytesMut};
use chrono::{Duration, Utc};
use futures::future::ok;
use futures::sync::mpsc::{unbounded, UnboundedReceiver, UnboundedSender};
use futures::{Async, Future, Poll, Sink, Stream};
use ilp::{IlpFulfill, IlpPacket, IlpPrepare, IlpReject, PacketType};
use num_bigint::BigUint;
use plugin::IlpRequest;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use tokio;

#[derive(Clone)]
pub struct DataMoneyStream {
  id: u64,
  conn: Connection,
  total_sent: Arc<AtomicUsize>,
  total_delivered: Arc<AtomicUsize>,
}

impl DataMoneyStream {
  pub fn total_sent(&self) -> u64 {
    self.total_sent.load(Ordering::SeqCst) as u64
  }

  pub fn total_delivered(&self) -> u64 {
    self.total_delivered.load(Ordering::SeqCst) as u64
  }

  pub fn send_money(&mut self, amount: u64) -> impl Future<Item = u64, Error = ()> {
    let total_sent = Arc::clone(&self.total_sent);
    let total_delivered = Arc::clone(&self.total_delivered);

    self
      .conn
      .send_money(self.id, amount)
      .and_then(move |amount_delivered| {
        total_sent.fetch_add(amount as usize, Ordering::SeqCst);
        total_delivered.fetch_add(amount_delivered as usize, Ordering::SeqCst);
        Ok(amount_delivered)
      })
  }

  pub fn close(&mut self) -> impl Future<Item = (), Error = ()> {
    self.conn.close_stream(self.id)
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
  // TODO add connection-level stats
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
    next_request_id: Arc<AtomicUsize>,
  ) -> Self {
    let next_stream_id = match is_server {
      true => 2,
      false => 1,
    };

    let (sender, receiver) = unbounded::<IlpRequest>();
    let queue_incoming = incoming
      .forward(sender.clone().sink_map_err(|err| {
        error!("Error forwarding request to queue {:?}", err);
      })).then(|_| Ok(()));
    tokio::spawn(queue_incoming);

    let conn = Connection {
      state: Arc::new(RwLock::new(ConnectionState::Opening)),
      outgoing,
      incoming: Arc::new(Mutex::new(receiver)),
      incoming_queue: sender.clone(),
      shared_secret,
      source_account: Arc::new(source_account),
      destination_account: Arc::new(destination_account),
      next_stream_id: Arc::new(AtomicUsize::new(next_stream_id)),
      next_packet_sequence: Arc::new(AtomicUsize::new(1)),
      next_request_id,
    };

    // TODO figure out a better way to send the initial packet - get the exchange rate and wait for response
    if !is_server {
      conn.send_handshake();
    }

    conn
  }

  pub fn send_money(&mut self, stream_id: u64, amount: u64) -> impl Future<Item = u64, Error = ()> {
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

    self
      .send_packet(amount, stream_packet)
      .and_then(|response: StreamPacket| Ok(response.prepare_amount))
  }

  pub fn close(&mut self) -> impl Future<Item = (), Error = ()> {
    {
      let mut state = self.state.write().unwrap();
      *state = ConnectionState::Closing;
    }

    let stream_packet = StreamPacket {
      sequence: self.next_packet_sequence.fetch_add(1, Ordering::SeqCst) as u64,
      ilp_packet_type: PacketType::IlpPrepare,
      prepare_amount: 0,
      frames: vec![Frame::ConnectionClose(ConnectionCloseFrame {
        code: ErrorCode::NoError,
        message: String::new(),
      })],
    };

    let state = Arc::clone(&self.state);
    self.send_packet(0, stream_packet).and_then(move |_| {
      let mut state = state.write().unwrap();
      *state = ConnectionState::Closed;
      Ok(())
    })
  }

  pub fn close_stream(&mut self, stream_id: u64) -> impl Future<Item = (), Error = ()> {
    let stream_packet = StreamPacket {
      sequence: self.next_packet_sequence.fetch_add(1, Ordering::SeqCst) as u64,
      ilp_packet_type: PacketType::IlpPrepare,
      prepare_amount: 0,
      frames: vec![Frame::StreamClose(StreamCloseFrame {
        stream_id: BigUint::from(stream_id),
        code: ErrorCode::NoError,
        message: String::new(),
      })],
    };

    self.send_packet(0, stream_packet).map(|_| ())
  }

  fn send_packet(
    &mut self,
    amount: u64,
    packet: StreamPacket,
  ) -> impl Future<Item = StreamPacket, Error = ()> {
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

    let shared_secret = self.shared_secret.clone();
    let expected_sequence = packet.sequence.clone();
    let request_id = request_id.clone();

    let wait_for_response = self.wait_for_response(request_id);
    self
      .outgoing
      .clone()
      .send(request)
      .map(|_| ())
      .map_err(move |err| {
        error!(
          "Error sending outgoing packet with request id {}: {:?} {:?}",
          request_id.clone(),
          err,
          packet
        );
      }).and_then(|_| wait_for_response)
      .and_then(move |(request_id, response)| {
        if let IlpPacket::Fulfill(fulfill) = response {
          // Parse and validate the stream packet in the response
          match StreamPacket::from_encrypted(shared_secret, BytesMut::from(fulfill.data)) {
            Ok(stream_packet) => {
              if stream_packet.sequence != expected_sequence {
                error!(
                "Stream packet response does not match request sequence. Expected: {}, actual: {}",
                expected_sequence, stream_packet.sequence
              );
                Err(())
              } else if stream_packet.ilp_packet_type != PacketType::IlpFulfill {
                error!(
                  "Stream packet response has the wrong packet type. Expected: {}, actual: {}",
                  PacketType::IlpFulfill as u8,
                  stream_packet.ilp_packet_type as u8
                );
                Err(())
              } else {
                debug!(
                  "Parsed stream packet from Fulfill {}: {:?}",
                  request_id, stream_packet
                );
                Ok(stream_packet)
              }
            }
            Err(err) => {
              error!(
                "Got invalid stream packet in valid Fulfill. Request ID: {}, {:?}",
                request_id, err
              );
              Err(())
            }
          }
        } else if let IlpPacket::Reject(reject) = response {
          // TODO handle stream packet
          error!("Request {} was rejected: {:?}", request_id, reject);
          Err(())
        } else {
          Err(())
        }
      })
  }

  fn wait_for_response(&mut self, request_id: u32) -> impl Future<Item = IlpRequest, Error = ()> {
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
      total_sent: Arc::new(AtomicUsize::new(0)),
      total_delivered: Arc::new(AtomicUsize::new(0)),
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

  // TODO wait for response
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
  type Item = IlpRequest;
  type Error = ();

  fn poll(&mut self) -> Poll<Self::Item, Self::Error> {
    if let Ok(ref mut incoming) = self.incoming.try_lock() {
      let item = try_ready!(incoming.poll().map_err(|err| {
        error!("Error polling incoming {:?}", err);
      }));
      if let Some((request_id, packet)) = item {
        // TODO check that the response is valid
        if request_id == self.request_id {
          Ok(Async::Ready((request_id, packet)))
        } else {
          self
            .incoming_queue
            .clone() // TODO avoid cloning this?
            .unbounded_send((request_id, packet))
            .map_err(|err| {
              error!("Error re-queuing request {} {:?}", request_id, err);
            })?;
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
