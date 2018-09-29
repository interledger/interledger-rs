use super::connection::{Connection, ConnectionInternal};
use futures::{Async, Poll, Sink, Stream, StartSend, AsyncSink};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use tokio_io::{AsyncWrite, AsyncRead};
use std::io::{Read, Write, ErrorKind, Error as IoError};
use std::collections::{VecDeque, HashMap};
use bytes::{Bytes, BytesMut, BufMut};

#[derive(PartialEq)]
enum StreamState {
  Open,
  Closing,
  Closed,
}

#[derive(Clone)]
pub struct DataMoneyStream {
  pub id: u64,
  pub money: MoneyStream,
  pub data: DataStream,
  state: Arc<RwLock<StreamState>>,
  connection: Arc<Connection>,
}

impl DataMoneyStream {
  pub fn close(&mut self) -> Result<Async<()>, ()> {
    if *self.state.read().unwrap() == StreamState::Closed {
      Ok(Async::Ready(()))
    } else {
      *self.state.write().unwrap() = StreamState::Closing;
      self.connection.try_send()?;
      Ok(Async::NotReady)
    }
  }
}

pub trait DataMoneyStreamInternal {
  fn new(id: u64, connection: Connection) -> DataMoneyStream;
}

impl DataMoneyStreamInternal for DataMoneyStream {
  fn new(id: u64, connection: Connection) -> DataMoneyStream {
    let connection = Arc::new(connection);
    DataMoneyStream {
      id,
      money: MoneyStream {
        connection: Arc::clone(&connection),
        send_max: Arc::new(AtomicUsize::new(0)),
        pending: Arc::new(AtomicUsize::new(0)),
        sent: Arc::new(AtomicUsize::new(0)),
        delivered: Arc::new(AtomicUsize::new(0)),
        received: Arc::new(AtomicUsize::new(0)),
        last_reported_received: Arc::new(AtomicUsize::new(0)),
      },
      data: DataStream {
        connection: Arc::clone(&connection),
        incoming: Arc::new(Mutex::new(IncomingData {
          offset: 0,
          buffer: HashMap::new(),
        })),
        outgoing: Arc::new(Mutex::new(OutgoingData {
          offset: 0,
          buffer: VecDeque::new(),
        }))
      },
      state: Arc::new(RwLock::new(StreamState::Open)),
      connection: Arc::clone(&connection),
    }
  }
}

#[derive(Clone)]
pub struct MoneyStream {
  connection: Arc<Connection>,
  send_max: Arc<AtomicUsize>,
  pending: Arc<AtomicUsize>,
  sent: Arc<AtomicUsize>,
  delivered: Arc<AtomicUsize>,
  received: Arc<AtomicUsize>,
  last_reported_received: Arc<AtomicUsize>,
}

impl MoneyStream {
  pub fn total_sent(&self) -> u64 {
    self.sent.load(Ordering::SeqCst) as u64
  }

  pub fn total_delivered(&self) -> u64 {
    self.delivered.load(Ordering::SeqCst) as u64
  }

  pub fn total_received(&self) -> u64 {
    self.received.load(Ordering::SeqCst) as u64
  }
}

impl Stream for MoneyStream {
  type Item = u64;
  type Error = ();

  fn poll(&mut self) -> Poll<Option<Self::Item>, Self::Error> {
    self.connection.try_handle_incoming()?;

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
    self.connection.try_send()?;
    Ok(AsyncSink::Ready)
  }

  fn poll_complete(&mut self) -> Poll<(), Self::SinkError> {
    self.connection.try_send()?;
    self.connection.try_handle_incoming()?;

    if self.sent.load(Ordering::SeqCst) >= self.send_max.load(Ordering::SeqCst) {
      Ok(Async::Ready(()))
    } else {
      trace!("No more money available for now");
      Ok(Async::NotReady)
    }
  }
}

// Used by the Connection
pub trait MoneyStreamInternal {
  fn pending(&self) -> u64;
  fn add_to_pending(&self, amount: u64);
  fn subtract_from_pending(&self, amount: u64);
  fn pending_to_sent(&self, amount: u64);
  fn send_max(&self) -> u64;
  fn add_received(&self, amount: u64);
  fn add_delivered(&self, amount: u64);
}

impl MoneyStreamInternal for MoneyStream {
  fn pending(&self) -> u64 {
    self.pending.load(Ordering::SeqCst) as u64
  }

  fn add_to_pending(&self, amount: u64) {
    self.pending.fetch_add(amount as usize, Ordering::SeqCst);
  }

  fn subtract_from_pending(&self, amount: u64) {
    self.pending.fetch_sub(amount as usize, Ordering::SeqCst);
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

  fn add_delivered(&self, amount: u64) {
    self.delivered.fetch_add(amount as usize, Ordering::SeqCst);
  }

  // pub fn close(&mut self) -> impl Future<Item = (), Error = ()> {
  //   self.conn.close_stream(self.id)
  // }
}

#[derive(Clone)]
pub struct DataStream {
  connection: Arc<Connection>,
  incoming: Arc<Mutex<IncomingData>>,
  outgoing: Arc<Mutex<OutgoingData>>,
}

struct IncomingData {
  offset: usize,
  // TODO should we allow duplicate bytes and let the other side to resize chunks of data?
  // (we would need a sorted list instead of a HashMap to allow this)
  buffer: HashMap<usize, Bytes>,
}

struct OutgoingData {
  offset: usize,
  buffer: VecDeque<Bytes>,
}

impl Read for DataStream {
  fn read(&mut self, buf: &mut [u8]) -> Result<usize, IoError> {
    if buf.len() == 0 {
      warn!("Asked to read into zero-length buffer");
    }

    self.connection.try_handle_incoming()
      .map_err(|_| {
        IoError::new(ErrorKind::Other, "Error trying to handle incoming packets on Connection")
      })?;

    if let Ok(mut incoming) = self.incoming.try_lock() {
      // let mut incoming = incoming.deref_mut();
      let incoming_offset = incoming.offset;
      if let Some(mut from_buf) = incoming.buffer.remove(&incoming_offset) {
        if from_buf.len() >= buf.len() {
          let to_copy = from_buf.split_to(buf.len());
          buf.copy_from_slice(&to_copy[..]);
          incoming.offset += to_copy.len();

          // Put the rest back in the queue
          if from_buf.len() > 0 {
            let incoming_offset = incoming.offset;
            incoming.buffer.insert(incoming_offset, from_buf);
          }

          incoming.offset += to_copy.len();
          Ok(to_copy.len())
        } else {
          let (mut buf_slice, _rest) = buf.split_at_mut(from_buf.len());
          buf_slice.copy_from_slice(&from_buf[..]);
          incoming.offset += from_buf.len();
          Ok(from_buf.len())
        }
      // TODO properly handle stream closing
      // } else if self.stream.state == StreamState::Closed {
      //   Ok(0 as usize)
      } else {
        Err(IoError::new(ErrorKind::WouldBlock, "No more data now but there might be more in the future"))
      }
    } else {
      warn!("Unable to get lock on incoming");
      Err(IoError::new(ErrorKind::WouldBlock, "Unable to get lock on incoming"))
    }
  }
}
impl AsyncRead for DataStream {}

impl Write for DataStream {
  fn write(&mut self, buf: &[u8]) -> Result<usize, IoError> {
    // TODO limit buffer size
    if let Ok(mut outgoing) = self.outgoing.try_lock() {
      outgoing.buffer.push_back(Bytes::from(buf));
    } else {
      return Err(IoError::new(ErrorKind::WouldBlock, "Unable to get lock on outgoing"))
    }

    self
      .connection
      .try_send()
      .map_err(|_| IoError::new(ErrorKind::Other, "Error trying to send through Connection"))?;
    Ok(buf.len())
  }

  fn flush(&mut self) -> Result<(), IoError> {
    // Try handling incoming packets in case the other side increased their limits
    self.connection.try_handle_incoming()
      .map_err(|_| {
        IoError::new(ErrorKind::Other, "Error trying to handle incoming packets on Connection")
      })?;

    self.connection.try_send()
      .map_err(|_| {
        IoError::new(ErrorKind::Other, "Error trying to send through Connection")
      })?;

    if let Ok(outgoing) = self.outgoing.try_lock() {
      if outgoing.buffer.len() == 0 {
        Ok(())
      } else {
        Err(IoError::new(ErrorKind::WouldBlock, "Not finished sending yet"))
      }
    } else {
      Err(IoError::new(ErrorKind::WouldBlock, "Unable to get lock on outgoing"))
    }
  }
}
impl AsyncWrite for DataStream {
  fn shutdown(&mut self) -> Result<Async<()>, IoError> {
    // TODO implement closing
    Ok(Async::Ready(()))
  }
}

pub trait DataStreamInternal {
  // TODO error if the buffer is too full
  fn push_incoming_data(&self, data: Bytes, offset: usize) -> Result<(), ()>;
  fn get_outgoing_data(&self, max_size: usize) -> Option<(Bytes, usize)>;
}

impl DataStreamInternal for DataStream {
  fn push_incoming_data(&self, data: Bytes, offset: usize) -> Result<(), ()> {
    // TODO don't block
    self.incoming.lock().unwrap().buffer.insert(offset, data);
    Ok(())
  }

  fn get_outgoing_data(&self, max_size: usize) -> Option<(Bytes, usize)> {
    let mut outgoing = {
      if let Ok(lock) = self.outgoing.try_lock() {
        lock
      } else {
      warn!("Unable to get lock on outgoing to get outgoing data");
      return None;
      }
    };

    // TODO make sure we're not copying data here
    let outgoing_offset = outgoing.offset;
    let mut chunks: Vec<Bytes> = Vec::new();
    let mut size: usize = 0;
    while size < max_size && !outgoing.buffer.is_empty() {
      let mut chunk = outgoing.buffer.pop_front().unwrap();
      if chunk.len() >= max_size - size {
        chunks.push(chunk.split_to(max_size - size));
        size = max_size;

        if chunk.len() > 0 {
          outgoing.buffer.push_front(chunk);
        }
      } else {
        size += chunk.len();
        chunks.push(chunk);
      }
    }

    if chunks.len() > 0 {
      // TODO zero copy
      let mut data = BytesMut::with_capacity(size);
      for chunk in chunks.iter() {
        data.put(chunk);
      }

      outgoing.offset += size;
      Some((data.freeze(), outgoing_offset))
    } else {
      None
    }
  }
}
