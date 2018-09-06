use std::error::{Error as StdError};
use futures::{Stream, Sink, Async, AsyncSink, StartSend, Poll};
use ilp_packet::{IlpPacket, Serializable};
use util::IlpOrBtpPacket;
use std::collections::HashMap;
use ring::digest::{digest, SHA256};
use chrono::{DateTime, Utc};

pub struct IlpFulfillmentChecker<S> {
  inner: S,
  packets: HashMap<u32, (Vec<u8>, DateTime<Utc>)>,
}

impl<S> IlpFulfillmentChecker<S>
where
  S: Stream<Item = IlpOrBtpPacket, Error = ()> + Sink<SinkItem = IlpOrBtpPacket, SinkError = ()>,
{
  pub fn new(stream: S) -> Self {
    IlpFulfillmentChecker {
      inner: stream,
      packets: HashMap::new(),
    }
  }
}

impl<S> Stream for IlpFulfillmentChecker<S>
where
  S: Stream<Item = IlpOrBtpPacket, Error = ()> + Sink<SinkItem = IlpOrBtpPacket, SinkError = ()>,
{
  type Item = IlpOrBtpPacket;
  type Error = ();

  fn poll(&mut self) -> Poll<Option<Self::Item>, Self::Error> {
    let item = try_ready!(self.inner.poll());
    match item {
      Some(IlpOrBtpPacket::Ilp(request_id, IlpPacket::Fulfill(fulfill))) => {
        if let Some((condition, expires_at)) = self.packets.get(&request_id) {
          if fulfillment_matches_condition(&fulfill.fulfillment, condition)
            && expires_at >= &Utc::now() {
            Ok(Async::Ready(Some(IlpOrBtpPacket::Ilp(request_id, IlpPacket::Fulfill(fulfill)))))
          } else {
            // Fulfillment doesn't match or is expired
            Ok(Async::NotReady)
          }
        } else {
          // We never saw the Prepare that corresponds to this
          Ok(Async::NotReady)
        }
      },
      Some(item) => {
        Ok(Async::Ready(Some(item)))
      },
      None => {
        Ok(Async::Ready(None))
      }
    }
  }
}

impl<S> Sink for IlpFulfillmentChecker<S>
where
  S: Sink<SinkItem = IlpOrBtpPacket, SinkError = ()>,
{
  type SinkItem = IlpOrBtpPacket;
  type SinkError = ();

  fn start_send(&mut self, item: Self::SinkItem) -> StartSend<Self::SinkItem, Self::SinkError> {
    if let IlpOrBtpPacket::Ilp(request_id, ilp) = &item {
      if let IlpPacket::Prepare(prepare) = &ilp {
        self
          .packets
          .insert(*request_id, (prepare.execution_condition.to_vec(), prepare.expires_at.clone()));
      }
    }

    self.inner.start_send(item)
  }

  fn poll_complete(&mut self) -> Poll<(), Self::SinkError> {
      self.inner.poll_complete()
    }
}

fn fulfillment_matches_condition(fulfillment: &[u8], condition: &[u8]) -> bool {
  digest(&SHA256, fulfillment).as_ref() == condition
}
