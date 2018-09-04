use std::error::{Error as StdError};
use futures::{Stream, Sink, Async, AsyncSink, StartSend, Poll};
use ilp_packet::{IlpPacket, Serializable};
use btp_packet::{BtpPacket, BtpMessage, BtpResponse, ProtocolData, ContentType};
use util::IlpOrBtpPacket;

pub struct IlpPacketStream<S> {
  inner: S,
}

impl<S> Stream for IlpPacketStream<S>
where
  S: Stream<Item = BtpPacket, Error = ()>,
{
  type Item = IlpOrBtpPacket;
  type Error = ();

  fn poll(&mut self) -> Poll<Option<Self::Item>, Self::Error> {
    if let Some(packet) = try_ready!(self.inner.poll()) {
      if let BtpPacket::Message(packet) = &packet {
          if packet.protocol_data.len() > 0 && packet.protocol_data[0].protocol_name == "ilp" {
            let parsed_ilp_packet = IlpPacket::from_bytes(&packet.protocol_data[0].data).map_err(|e| {})?;
            return Ok(Async::Ready(Some(IlpOrBtpPacket::Ilp(packet.request_id, parsed_ilp_packet))))
          }
      } else if let BtpPacket::Response(packet) = &packet {
          if packet.protocol_data.len() > 0 && packet.protocol_data[0].protocol_name == "ilp" {
            let parsed_ilp_packet = IlpPacket::from_bytes(&packet.protocol_data[0].data).map_err(|e| {})?;
            return Ok(Async::Ready(Some(IlpOrBtpPacket::Ilp(packet.request_id, parsed_ilp_packet))))
          }
      }

      Ok(Async::Ready(Some(IlpOrBtpPacket::Btp(packet))))
    } else {
      Ok(Async::Ready(None))
    }
  }
}

impl<S> Sink for IlpPacketStream<S>
where
  S: Sink<SinkItem = BtpPacket, SinkError = ()>,
{
  type SinkItem = IlpOrBtpPacket;
  type SinkError = ();

  fn start_send(&mut self, item: Self::SinkItem) -> StartSend<Self::SinkItem, Self::SinkError> {
    // TODO there must be a better way of passing this back in case the Sink isn't ready
    let item_clone = item.clone();
    let btp_packet_to_send = match item {
      IlpOrBtpPacket::Ilp(request_id, IlpPacket::Prepare(ref packet)) => BtpPacket::Message(BtpMessage {
        request_id,
        protocol_data: vec![ProtocolData {
          protocol_name: String::from("ilp"),
          content_type: ContentType::ApplicationOctetStream,
          data: packet.to_bytes().unwrap()
        }]
      }),
      IlpOrBtpPacket::Ilp(request_id, packet) => BtpPacket::Response(BtpResponse {
        request_id,
        protocol_data: vec![ProtocolData {
          protocol_name: String::from("ilp"),
          content_type: ContentType::ApplicationOctetStream,
          data: packet.to_bytes().unwrap()
        }]
      }),
      IlpOrBtpPacket::Btp(packet) => packet,
    };
    self
      .inner
      .start_send(btp_packet_to_send)
      .map(move |result| match result {
        AsyncSink::Ready => AsyncSink::Ready,
        AsyncSink::NotReady(_) => AsyncSink::NotReady(item_clone),
      })
      .map_err(|err| {
        // println!("Error sending {}", err);
      })
  }

  fn poll_complete(&mut self) -> Poll<(), Self::SinkError> {
      self.inner.poll_complete().map_err(|e| {
        // println!("Polling error {}", e);
      })
    }
}
