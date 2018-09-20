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
      match packet {
        BtpPacket::Message(message) => Ok(Async::Ready(Some(BtpPacket::Message(message)))),
        BtpPacket::Response(response) => {
          if self.outgoing_ids.remove(&response.request_id) {
            Ok(Async::Ready(Some(BtpPacket::Response(response))))
          } else {
            trace!("Ignoring BTP packet because there is no pending request with that ID {:?}", response);
            Ok(Async::NotReady)
          }
        },
        BtpPacket::Error(error) => {
          if self.outgoing_ids.remove(&error.request_id) {
            Ok(Async::Ready(Some(BtpPacket::Error(error))))
          } else {
            trace!("Ignoring BTP packet because there is no pending request with that ID {:?}", error);
            Ok(Async::NotReady)
          }
        },
        _ => {
          debug!("Ignoring unexpected BTP packet type {:?}", packet);
          Ok(Async::NotReady)
        }
      }
    } else {
      trace!("Stream ended");
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
      if self.outgoing_ids.insert(message.request_id) {
        trace!("Storing outgoing request ID {}", message.request_id);
      } else {
        trace!("Duplicate request ID {}", message.request_id);
        return Err(())
      }
    }

    self.inner.start_send(item)
  }

  fn poll_complete(&mut self) -> Result<Async<()>, Self::SinkError> {
    self.inner.poll_complete()
  }
}