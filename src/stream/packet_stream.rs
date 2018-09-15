use super::crypto::{fulfillment_to_condition, generate_fulfillment};
use super::StreamPacket;
use bytes::Bytes;
use futures::{Async, AsyncSink, Poll, Sink, StartSend, Stream};
use ilp::{IlpPacket, IlpReject};

pub struct StreamPacketStream<S> {
  shared_secret: Bytes,
  inner: S,
}

impl<S> StreamPacketStream<S>
where
  S:
    Stream<Item = (u32, IlpPacket), Error = ()> + Sink<SinkItem = (u32, IlpPacket), SinkError = ()>,
{
  pub fn new(shared_secret: Bytes, stream: S) -> Self {
    StreamPacketStream {
      shared_secret,
      inner: stream,
    }
  }
}

impl<S> Stream for StreamPacketStream<S>
where
  S:
    Stream<Item = (u32, IlpPacket), Error = ()> + Sink<SinkItem = (u32, IlpPacket), SinkError = ()>,
{
  type Item = (u32, IlpPacket, StreamPacket);
  type Error = ();

  fn poll(&mut self) -> Poll<Option<Self::Item>, Self::Error> {
    if let Some((request_id, packet)) = try_ready!(self.inner.poll()) {
      let parse_result = match packet {
        IlpPacket::Prepare(ref packet) => {
          let fulfillment = generate_fulfillment(&self.shared_secret[..], &packet.data[..]);
          let condition = fulfillment_to_condition(&fulfillment);
          if condition != packet.execution_condition {
            warn!("Got ILP packet where the condition does not match the one we generate from the data. Expected: {:?}, packet: {:?}", &condition[..], packet);
            // TODO reject packet
            return Ok(Async::NotReady);
          }
          StreamPacket::from_encrypted(&self.shared_secret[..], &packet.data[..])
        }
        IlpPacket::Fulfill(ref packet) => {
          StreamPacket::from_encrypted(&self.shared_secret[..], &packet.data[..])
        }
        IlpPacket::Reject(ref packet) => {
          StreamPacket::from_encrypted(&self.shared_secret[..], &packet.data[..])
        }
        IlpPacket::Unknown => {
          warn!("Got ILP packet with no data: {:?}", packet);
          // TODO reject packet
          // self.inner.start_send((request_id, IlpPacket::Reject(IlpReject::new("F06", "", "", Bytes::new()))));
          return Ok(Async::NotReady);
        }
      };
      if let Ok(stream_packet) = parse_result {
        Ok(Async::Ready(Some((request_id, packet, stream_packet))))
      } else {
        warn!("Got ILP packet with data we cannot parse: {:?}", packet);
        // TODO reject packet
        // self.inner.start_send((request_id, IlpPacket::Reject(IlpReject::new("F06", "", "", Bytes::new()))))
        //   .map_err(|_| {
        //     warn!("Error rejecting packet {}", request_id);
        //   });
        Ok(Async::NotReady)
      }
    } else {
      Ok(Async::Ready(None))
    }
  }
}

impl<S> Sink for StreamPacketStream<S>
where
  S: Sink<SinkItem = (u32, IlpPacket), SinkError = ()>,
{
  type SinkItem = (u32, IlpPacket, StreamPacket);
  type SinkError = ();

  fn start_send(&mut self, item: Self::SinkItem) -> StartSend<Self::SinkItem, Self::SinkError> {
    let (request_id, packet, stream_packet) = item;
    let encrypted = stream_packet.to_encrypted(&self.shared_secret[..]).unwrap();
    let packet = match packet {
      IlpPacket::Prepare(mut packet) => {
        let fulfillment = generate_fulfillment(&self.shared_secret[..], encrypted.as_slice());
        let condition = fulfillment_to_condition(&fulfillment);
        packet.data = Bytes::from(encrypted);
        packet.execution_condition = condition;
        IlpPacket::Prepare(packet)
      }
      IlpPacket::Fulfill(mut packet) => {
        let fulfillment = generate_fulfillment(&self.shared_secret[..], encrypted.as_slice());
        packet.data = Bytes::from(encrypted);
        packet.fulfillment = fulfillment;
        IlpPacket::Fulfill(packet)
      }
      IlpPacket::Reject(mut packet) => {
        packet.data = Bytes::from(encrypted);
        IlpPacket::Reject(packet)
      }
      IlpPacket::Unknown => return Err(()),
    };

    self
      .inner
      .start_send((request_id, packet))
      .map(|result| match result {
        AsyncSink::Ready => AsyncSink::Ready,
        AsyncSink::NotReady((request_id, packet)) => AsyncSink::NotReady((request_id, packet, stream_packet)),
      })
  }

  fn poll_complete(&mut self) -> Result<Async<()>, Self::SinkError> {
    self.inner.poll_complete()
  }
}
