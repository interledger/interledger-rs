pub use btp_packet::ContentType; // reexport
use btp_packet::{
  deserialize_packet, BtpMessage, BtpPacket, BtpResponse, ProtocolData, Serializable,
};
use bytes::{Bytes, BytesMut};
use futures::Sink;
use futures::stream::{SplitSink, SplitStream, Stream};
use futures::{Async, AsyncSink, Future, Poll, StartSend};
use std::error::Error as StdError;
use tokio_tcp::TcpStream;
use tokio_tungstenite::{connect_async as connect_websocket, MaybeTlsStream, WebSocketStream};
use tungstenite::{Error as WebSocketError, Message as WebSocketMessage};
use url::{ParseError, Url};
use futures::future::{ok, done};

pub type BtpStream = BtpPacketStream<WebSocketStream<MaybeTlsStream<TcpStream>>>;

pub struct BtpPacketStream<S> {
  inner: S,
}

impl<S> Stream for BtpPacketStream<S>
where
  S: Stream,
  S::Item: Into<Vec<u8>>,
  S::Error: StdError,
{
  type Item = BtpPacket;
  type Error = ();

  fn poll(&mut self) -> Poll<Option<BtpPacket>, Self::Error> {
    let poll_result = self.inner.poll().map_err(|err| {
      error!("Error polling: {:?}", err)
    });
    if let Some(serialized) = try_ready!(poll_result) {
      let serialized_vec: Vec<u8> = serialized.into();
      trace!("Got packet: {:?}", &serialized_vec);
      if let Ok(packet) = deserialize_packet(&serialized_vec) {
      trace!("Parsed BTP packet: {:?}", packet.clone());
        Ok(Async::Ready(Some(packet)))
      } else {
        warn!("Ignoring unknown BTP packet {:x?}", &serialized_vec);
        Ok(Async::NotReady)
      }
    } else {
      Ok(Async::Ready(None))
    }
  }
}

impl<S> Sink for BtpPacketStream<S>
where
  S: Sink,
  S::SinkItem: From<Vec<u8>>,
  S::SinkError: StdError,
{
  type SinkItem = BtpPacket;
  type SinkError = ();

  fn start_send(&mut self, item: BtpPacket) -> StartSend<Self::SinkItem, Self::SinkError> {
    trace!("Sending BTP packet: {:?}", item.clone());
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

pub fn connect_async(server: &str) -> impl Future<Item = BtpStream, Error = ()> + 'static + Send {
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
  let mut server_without_auth = server.clone();
  server_without_auth.set_username("").unwrap();
  server_without_auth.set_password(None).unwrap();
  debug!("Connecting WebSocket: {}", &server_without_auth);
  connect_websocket(server.clone())
    .map_err(|err| {
      error!("Error connecting to websocket: {:?}", err);
    })
    .and_then(|(ws, _handshake)| Ok(BtpPacketStream { inner: ws }))
    .and_then(move |plugin| {
      plugin
        .send(auth_packet)
        .map_err(|err| {
          error!("Error sending auth packet: {:?}", err);
        })
        .and_then(move |plugin| {
          plugin
            .into_future()
            .and_then(move |(_auth_response, plugin)| {
              info!("Connected to server: {}", server_without_auth);
              Ok(plugin)
            })
            .map_err(|(err, _plugin)| error!("Error getting auth response: {:?}", err))
        })
    })
}
