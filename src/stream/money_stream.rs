use super::connection::{Connection, ConnectionInternal};
use futures::{Async, Poll, Sink, Stream, StartSend, AsyncSink};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

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
  pub fn id(&self) -> u64 {
    self.id
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

// Used by the Connection
pub trait MoneyStreamInternal {
  fn new(id: u64, connection: Connection) -> Self;
  fn pending(&self) -> u64;
  fn add_to_pending(&self, amount: u64);
  fn subtract_from_pending(&self, amount: u64);
  fn pending_to_sent(&self, amount: u64);
  fn send_max(&self) -> u64;
  fn add_received(&self, amount: u64);
  fn add_delivered(&self, amount: u64);
}

impl MoneyStreamInternal for MoneyStream {
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
