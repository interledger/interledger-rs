use super::crypto::{fulfillment_to_condition, generate_fulfillment};
use super::StreamPacket;
use bytes::Bytes;
use futures::stream::Fuse;
use futures::{Async, AsyncSink, Poll, Sink, StartSend, Stream};
use ilp::{IlpPacket, IlpReject};

pub struct StreamPacketStream<T: Stream, U> {
  shared_secret: Bytes,
  sink: Option<U>,
  stream: Option<Fuse<T>>,
  buffered: Option<T::Item>,
}

impl<T, U> StreamPacketStream<T, U>
where
  T: Stream<Item = (u32, IlpPacket), Error = ()>,
  U: Sink<SinkItem = (u32, IlpPacket), SinkError = ()>,
{
  pub fn new(shared_secret: Bytes, stream: T, sink: U) -> Self {
    StreamPacketStream {
      shared_secret,
      stream: Some(stream.fuse()),
      sink: Some(sink),
      buffered: None,
    }
  }

  fn sink_mut(&mut self) -> Option<&mut U> {
    self.sink.as_mut()
  }

  fn stream_mut(&mut self) -> Option<&mut T> {
    self.stream.as_mut().map(|x| x.get_mut())
  }

  fn try_start_send(&mut self, item: T::Item) -> Poll<(), U::SinkError> {
    debug_assert!(self.buffered.is_none());
    if let AsyncSink::NotReady(item) = self
      .sink_mut()
      .take()
      .expect("Attempted to poll after completion")
      .start_send(item)?
    {
      self.buffered = Some(item);
      return Ok(Async::NotReady);
    }
    Ok(Async::Ready(()))
  }
}

impl<T, U> Stream for StreamPacketStream<T, U>
where
  T: Stream<Item = (u32, IlpPacket), Error = ()>,
  U: Sink<SinkItem = (u32, IlpPacket), SinkError = ()>,
{
  type Item = (u32, IlpPacket, Option<StreamPacket>);
  type Error = ();

  fn poll(&mut self) -> Poll<Option<Self::Item>, Self::Error> {
    // If we've got an item buffered already, we need to write it to the
    // sink before we can do anything else
    if let Some(item) = self.buffered.take() {
      self.try_start_send(item)?;
      return Ok(Async::NotReady);
    }

    let next_item = try_ready!(
      self
        .stream_mut()
        .take()
        .expect("Attempted to poll after completion")
        .poll()
    );

    if next_item.is_none() {
      return Ok(Async::Ready(None));
    }

    let (request_id, packet) = next_item.unwrap();

    // Try parsing the iLP packet data as a STREAM packet
    match packet {
      IlpPacket::Prepare(packet) => {
        // Check that the condition matches what we regenerate
        let fulfillment = generate_fulfillment(&self.shared_secret[..], &packet.data[..]);
        let condition = fulfillment_to_condition(&fulfillment);
        if condition != packet.execution_condition {
          // Reject the packet
          warn!("Got ILP packet where the condition does not match the one we generate from the data. Expected: {:?}, packet: {:?}", &condition[..], packet);
          let reject = (
            request_id,
            IlpPacket::Reject(IlpReject::new("F02", "", "", Bytes::new())),
          );
          self.try_start_send(reject)?;
          return Ok(Async::NotReady);
        }

        // Check if we can decrypt and parse the STREAM packet
        if let Ok(stream_packet) =
          StreamPacket::from_encrypted(&self.shared_secret[..], &packet.data[..])
        {
          Ok(Async::Ready(Some((
            request_id,
            IlpPacket::Prepare(packet),
            Some(stream_packet),
          ))))
        } else {
          // Reject the packet
          warn!("Got ILP packet with data we cannot parse: {:?}", packet);
          let reject = (
            request_id,
            IlpPacket::Reject(IlpReject::new("F06", "", "", Bytes::new())),
          );
          self.try_start_send(reject)?;
          Ok(Async::NotReady)
        }
      }
      IlpPacket::Fulfill(packet) => {
        // Check if we can decrypt and parse the STREAM packet
        if let Ok(stream_packet) =
          StreamPacket::from_encrypted(&self.shared_secret[..], &packet.data[..])
        {
          Ok(Async::Ready(Some((
            request_id,
            IlpPacket::Fulfill(packet),
            Some(stream_packet),
          ))))
        } else {
          warn!(
            "Got ILP Fulfill for request: {} with no data attached: {:?}",
            request_id, packet
          );
          Ok(Async::Ready(Some((
            request_id,
            IlpPacket::Fulfill(packet),
            None,
          ))))
        }
      }
      IlpPacket::Reject(packet) => {
        // Check if we can decrypt and parse the STREAM packet
        if let Ok(stream_packet) =
          StreamPacket::from_encrypted(&self.shared_secret[..], &packet.data[..])
        {
          Ok(Async::Ready(Some((
            request_id,
            IlpPacket::Reject(packet),
            Some(stream_packet),
          ))))
        } else {
          Ok(Async::Ready(Some((
            request_id,
            IlpPacket::Reject(packet),
            None,
          ))))
        }
      }
      IlpPacket::Unknown => {
        warn!("Got ILP packet with no data: {:?}", packet);
        let reject = (
          request_id,
          IlpPacket::Reject(IlpReject::new("F06", "", "", Bytes::new())),
        );
        self.try_start_send(reject)?;
        Ok(Async::NotReady)
      }
    }
  }
}

impl<T, U> Sink for StreamPacketStream<T, U>
where
  T: Stream<Item = (u32, IlpPacket), Error = ()>,
  U: Sink<SinkItem = (u32, IlpPacket), SinkError = ()>,
{
  type SinkItem = (u32, IlpPacket, Option<StreamPacket>);
  type SinkError = ();

  fn start_send(&mut self, item: Self::SinkItem) -> StartSend<Self::SinkItem, Self::SinkError> {
    let (request_id, packet, stream_packet) = item;

    // TODO error if the ILP Packet data isn't empty

    // Replace the packet data with the encrypted STREAM packet
    let (packet, stream_packet) = if let Some(stream_packet) = stream_packet {
      let encrypted = stream_packet
        .to_encrypted(&self.shared_secret[..])
        .unwrap();

      match packet {
        IlpPacket::Prepare(mut packet) => {
          let fulfillment = generate_fulfillment(&self.shared_secret[..], encrypted.as_slice());
          let condition = fulfillment_to_condition(&fulfillment);
          packet.data = Bytes::from(encrypted);
          packet.execution_condition = condition;
          (IlpPacket::Prepare(packet), Some(stream_packet))
        }
        IlpPacket::Fulfill(mut packet) => {
          let fulfillment = generate_fulfillment(&self.shared_secret[..], encrypted.as_slice());
          packet.data = Bytes::from(encrypted);
          packet.fulfillment = fulfillment;
          (IlpPacket::Fulfill(packet), Some(stream_packet))
        }
        IlpPacket::Reject(mut packet) => {
          packet.data = Bytes::from(encrypted);
          (IlpPacket::Reject(packet), Some(stream_packet))
        }
        IlpPacket::Unknown => return Err(()),
      }
    } else {
      (packet, stream_packet)
    };

    self
      .sink_mut()
      .take()
      .expect("Attempted to send after completion")
      .start_send((request_id, packet))
      .map(|result| match result {
        AsyncSink::Ready => AsyncSink::Ready,
        AsyncSink::NotReady((request_id, packet)) => {
          AsyncSink::NotReady((request_id, packet, stream_packet))
        }
      })
  }

  fn poll_complete(&mut self) -> Result<Async<()>, Self::SinkError> {
    self
      .sink_mut()
      .take()
      .expect("Attempted to send after completion")
      .poll_complete()
  }
}
