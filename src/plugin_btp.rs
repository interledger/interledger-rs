pub use btp_packet::ContentType; // reexport
use btp_packet::{
  deserialize_packet, BtpMessage, BtpPacket, BtpResponse, ProtocolData, Serializable,
};
use bytes::{Bytes, BytesMut};
use futures::sink::Sink;
use futures::stream::{SplitSink, SplitStream, Stream};
use futures::{Async, AsyncSink, Future, Poll, StartSend};
use std::error::Error as StdError;
use tokio_tcp::TcpStream;
use tokio_tungstenite::{connect_async as connect_websocket, MaybeTlsStream, WebSocketStream};
use tungstenite::{Error as WebSocketError, Message as WebSocketMessage};
use url::{ParseError, Url};

pub struct PluginBtp<S> {
  inner: S,
}

impl<S> Stream for PluginBtp<S>
where
  S: Stream,
  S::Item: Into<Vec<u8>>,
  S::Error: StdError,
{
  type Item = BtpPacket;
  type Error = ();

  fn poll(&mut self) -> Poll<Option<BtpPacket>, Self::Error> {
    if let Some(serialized) = try_ready!(self.inner.poll().map_err(|e| {})) {
      let packet = deserialize_packet(&serialized.into()).unwrap();
      Ok(Async::Ready(Some(packet)))
    } else {
      Ok(Async::Ready(None))
    }
  }
}

impl<S> Sink for PluginBtp<S>
where
  S: Sink,
  S::SinkItem: From<Vec<u8>>,
  S::SinkError: StdError,
{
  type SinkItem = BtpPacket;
  type SinkError = ();

  fn start_send(&mut self, item: BtpPacket) -> StartSend<Self::SinkItem, Self::SinkError> {
    let serialized = item.to_bytes().unwrap();
    self
      .inner
      .start_send(serialized.into())
      .map(|result| match result {
        AsyncSink::Ready => AsyncSink::Ready,
        AsyncSink::NotReady(_) => AsyncSink::NotReady(item),
      })
      .map_err(|err| {
        println!("Error sending {}", err);
      })
  }

  fn poll_complete(&mut self) -> Result<Async<()>, Self::SinkError> {
    self.inner.poll_complete().map_err(|e| {
      println!("Polling error {}", e);
    })
  }
}

pub fn connect_async(
  server: &str,
) -> Box<
  Future<Item = PluginBtp<WebSocketStream<MaybeTlsStream<TcpStream>>>, Error = ()> + 'static + Send,
> {
  let server = Url::parse(server).unwrap();
  let auth_packet = BtpPacket::Message(BtpMessage {
    request_id: 0,
    protocol_data: vec![
      ProtocolData {
        protocol_name: String::from("auth"),
        content_type: ContentType::ApplicationOctetStream,
        data: vec![],
      },
      ProtocolData {
        protocol_name: String::from("auth_username"),
        content_type: ContentType::TextPlainUtf8,
        data: String::from(server.username()).into_bytes(),
      },
      ProtocolData {
        protocol_name: String::from("auth_token"),
        content_type: ContentType::TextPlainUtf8,
        data: String::from(server.password().unwrap()).into_bytes(),
      },
    ],
  });
  let future = connect_websocket(server.clone())
    .and_then(|(ws, _)| {
      // println!("Connected to ${}", &server);
      Ok(PluginBtp { inner: ws })
    })
    .map_err(|e| {
      println!("Error connecting: {}", e);
    })
    .and_then(move |plugin| {
      plugin.send(auth_packet)
    })
    .map_err(|e| {
      println!("Error sending auth message");
    });
  Box::new(future)
}
