use std::error::{Error as StdError};
use futures::{Stream, Sink, Async, AsyncSink, StartSend, Poll};
use ilp_packet::{IlpPacket, Serializable};
use btp_packet::{BtpPacket, BtpMessage, BtpResponse, ProtocolData, ContentType};
use util::OutgoingRequestIdGenerator;

pub struct IlpPacketStream<S> {
  inner: S,
  request_id_generator: OutgoingRequestIdGenerator,
}

pub enum IlpOrBtpPacket {
  Ilp(IlpPacket),
  Btp(BtpPacket),
}

impl<S> Stream for IlpPacketStream<S>
where
  S: Stream<Item = BtpPacket, Error = ()>,
{
  type Item = IlpOrBtpPacket;
  type Error = ();

  fn poll(&mut self) -> Poll<Option<Self::Item>, Self::Error> {
    if let Some(packet) = try_ready!(self.inner.poll()) {
      if let BtpPacket::Message(message) = packet {
          if message.protocol_data.len() > 0 && message.protocol_data[0].protocol_name == "ilp" {
            let parsed_ilp_packet = IlpPacket::from_bytes(&message.protocol_data[0].data).map_err(|e| {})?;
            return Ok(Async::Ready(Some(IlpOrBtpPacket::Ilp(parsed_ilp_packet))))
          }
      } else if let BtpPacket::Response(message) = packet {
          if message.protocol_data.len() > 0 && message.protocol_data[0].protocol_name == "ilp" {
            let parsed_ilp_packet = IlpPacket::from_bytes(&message.protocol_data[0].data).map_err(|e| {})?;
            return Ok(Async::Ready(Some(IlpOrBtpPacket::Ilp(parsed_ilp_packet))))
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
    let btp_packet_to_send = match item {
      IlpOrBtpPacket::Ilp(IlpPacket::Prepare(packet)) => BtpPacket::Message(BtpMessage {
        request_id: self.request_id_generator.get_next_id(),
        protocol_data: vec![ProtocolData {
          protocol_name: String::from("ilp"),
          content_type: ContentType::ApplicationOctetStream,
          data: packet.to_bytes().unwrap()
        }]
      }),
      IlpOrBtpPacket::Ilp(packet) => BtpPacket::Response(BtpResponse {
        request_id: self.request_id_generator.get_next_id(),
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
      .map(|result| match result {
        AsyncSink::Ready => AsyncSink::Ready,
        AsyncSink::NotReady(_) => AsyncSink::NotReady(item),
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
