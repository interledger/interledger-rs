use super::BtpPacket;
use futures::{Stream, Sink, Async, StartSend, Poll};
use std::collections::HashSet;

pub struct BtpRequestIdCheckerStream<S> {
  inner: S,
  outgoing_ids: HashSet<u32>,
}

impl<S> BtpRequestIdCheckerStream<S>
where
  S: Stream<Item = BtpPacket, Error = ()> + Sink<SinkItem = BtpPacket, SinkError = ()>,
{
  pub fn new(stream: S) -> Self {
    BtpRequestIdCheckerStream {
      inner: stream,
      outgoing_ids: HashSet::new(),
    }
  }
}

impl<S> Stream for BtpRequestIdCheckerStream<S>
where
  S: Stream<Item = BtpPacket, Error = ()> + Sink<SinkItem = BtpPacket, SinkError = ()>,
{
  type Item = BtpPacket;
  type Error = ();

  fn poll(&mut self) -> Poll<Option<BtpPacket>, Self::Error> {
    if let Some(packet) = try_ready!(self.inner.poll()) {
      if let BtpPacket::Response(response) = &packet {
        if !self.outgoing_ids.remove(&response.request_id) {
          return Ok(Async::NotReady)
        }
      } else if let BtpPacket::Error(error) = &packet {
        if !self.outgoing_ids.remove(&error.request_id) {
          return Ok(Async::NotReady)
        }
      }

      Ok(Async::Ready(Some(packet)))
    } else {
      Ok(Async::Ready(None))
    }
  }
}

impl<S> Sink for BtpRequestIdCheckerStream<S>
where
  S: Stream<Item = BtpPacket, Error = ()> + Sink<SinkItem = BtpPacket, SinkError = ()>,
{
  type SinkItem = BtpPacket;
  type SinkError = ();

  fn start_send(&mut self, item: BtpPacket) -> StartSend<Self::SinkItem, Self::SinkError> {
    if let BtpPacket::Message(message) = &item {
      // TODO check that this isn't a duplicate id
      self.outgoing_ids.insert(message.request_id);
    }

    self.inner.start_send(item)
  }

  fn poll_complete(&mut self) -> Result<Async<()>, Self::SinkError> {
    self.inner.poll_complete()
  }
}