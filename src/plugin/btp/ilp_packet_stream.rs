use super::super::super::ilp::{IlpPacket, Serializable};
use super::packet::{BtpMessage, BtpPacket, BtpResponse, ContentType, ProtocolData};
use futures::{Async, AsyncSink, Poll, Sink, StartSend, Stream};
use std::error::Error as StdError;

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
          let parsed_ilp_packet =
            IlpPacket::from_bytes(&packet.protocol_data[0].data).map_err(|e| {})?;
          trace!("Got ILP packet: {:?}", parsed_ilp_packet);
          return Ok(Async::Ready(Some((packet.request_id, parsed_ilp_packet))));
        }
      } else if let BtpPacket::Response(packet) = &packet {
        if packet.protocol_data.len() > 0 && packet.protocol_data[0].protocol_name == "ilp" {
          let parsed_ilp_packet =
            IlpPacket::from_bytes(&packet.protocol_data[0].data).map_err(|e| {})?;
          trace!("Got ILP packet: {:?}", parsed_ilp_packet);
          return Ok(Async::Ready(Some((packet.request_id, parsed_ilp_packet))));
        }
      }

      Ok(Async::NotReady)
    } else {
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
