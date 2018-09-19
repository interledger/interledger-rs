use super::packet::{BtpMessage, BtpPacket, BtpResponse, ContentType, ProtocolData};
use futures::{Async, AsyncSink, Poll, Sink, StartSend, Stream};
use ilp::{IlpPacket, Serializable};

pub struct IlpPacketStream<S> {
  inner: S,
}

impl<S> IlpPacketStream<S>
where
  S: Stream<Item = BtpPacket, Error = ()> + Sink<SinkItem = BtpPacket, SinkError = ()>,
{
  pub fn new(stream: S) -> Self {
    IlpPacketStream { inner: stream }
  }
}

impl<S> Stream for IlpPacketStream<S>
where
  S: Stream<Item = BtpPacket, Error = ()>,
{
  type Item = (u32, IlpPacket);
  type Error = ();

  fn poll(&mut self) -> Poll<Option<Self::Item>, Self::Error> {
    if let Some(packet) = try_ready!(self.inner.poll()) {
      if let BtpPacket::Message(packet) = &packet {
        if packet.protocol_data.len() > 0 && packet.protocol_data[0].protocol_name == "ilp" {
          if let Ok(parsed_ilp_packet) = IlpPacket::from_bytes(&packet.protocol_data[0].data) {
            trace!(
              "Parsed ILP packet: {} {:?}",
              packet.request_id,
              parsed_ilp_packet
            );
            Ok(Async::Ready(Some((packet.request_id, parsed_ilp_packet))))
          } else {
            trace!(
              "Unable to parse ILP packet from BTP packet protocol data: {:x?}",
              packet.protocol_data[0].data
            );
            // TODO is this problematic that we're returning NotReady even though the underlying stream returned ready?
            Ok(Async::NotReady)
          }
        } else {
          trace!(
            "Ignoring BTP packet that had no ILP packet in it {:?}",
            packet
          );
          Ok(Async::NotReady)
        }
      } else if let BtpPacket::Response(packet) = &packet {
        if packet.protocol_data.len() > 0 && packet.protocol_data[0].protocol_name == "ilp" {
          if let Ok(parsed_ilp_packet) = IlpPacket::from_bytes(&packet.protocol_data[0].data) {
            trace!(
              "Parsed ILP packet: {} {:?}",
              packet.request_id,
              parsed_ilp_packet
            );
            Ok(Async::Ready(Some((packet.request_id, parsed_ilp_packet))))
          } else {
            trace!(
              "Unable to parse ILP packet from BTP packet protocol data: {:x?}",
              packet.protocol_data[0].data
            );
            Ok(Async::NotReady)
          }
        } else {
          trace!(
            "Ignoring BTP packet that had no ILP packet in it {:?}",
            packet
          );
          Ok(Async::NotReady)
        }
      } else {
        trace!("Ignoring unexpected BTP packet {:?}", packet);
        Ok(Async::NotReady)
      }
    } else {
      trace!("Stream ended");
      Ok(Async::Ready(None))
    }
  }
}

impl<S> Sink for IlpPacketStream<S>
where
  S: Sink<SinkItem = BtpPacket, SinkError = ()>,
{
  type SinkItem = (u32, IlpPacket);
  type SinkError = ();

  fn start_send(&mut self, item: Self::SinkItem) -> StartSend<Self::SinkItem, Self::SinkError> {
    // TODO there must be a better way of passing this back in case the Sink isn't ready
    let item_clone = item.clone();
    let btp_packet_to_send = match item {
      (request_id, IlpPacket::Prepare(ref packet)) => {
        trace!(
          "Sending ILP packet with request id {}: {:?}",
          request_id,
          packet
        );
        BtpPacket::Message(BtpMessage {
          request_id,
          protocol_data: vec![ProtocolData {
            protocol_name: String::from("ilp"),
            content_type: ContentType::ApplicationOctetStream,
            data: packet.to_bytes().unwrap(),
          }],
        })
      }
      (request_id, packet) => {
        trace!(
          "Sending ILP packet with request id {}: {:?}",
          request_id,
          packet
        );
        BtpPacket::Response(BtpResponse {
          request_id,
          protocol_data: vec![ProtocolData {
            protocol_name: String::from("ilp"),
            content_type: ContentType::ApplicationOctetStream,
            data: packet.to_bytes().unwrap(),
          }],
        })
      }
    };
    self
      .inner
      .start_send(btp_packet_to_send)
      .map(move |result| match result {
        AsyncSink::Ready => AsyncSink::Ready,
        AsyncSink::NotReady(_) => AsyncSink::NotReady(item_clone),
      }).map_err(|err| {
        error!("Error sending packet {:?}", err);
      })
  }

  fn poll_complete(&mut self) -> Poll<(), Self::SinkError> {
    self.inner.poll_complete().map_err(|err| {
      error!("Error polling {:?}", err);
    })
  }
}
