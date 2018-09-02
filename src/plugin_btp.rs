use std::error::Error as StdError;
use btp_packet::{BtpMessage, BtpResponse, Serializable, ProtocolData, ContentType, deserialize_packet, BtpPacket};
use tokio_tungstenite::{connect_async as connect_websocket, WebSocketStream, MaybeTlsStream};
use tungstenite::{Error as WebSocketError, Message as WebSocketMessage};
use tokio_tcp::TcpStream;
use futures::{Future, Async, AsyncSink};
use futures::stream::{Stream, SplitSink, SplitStream};
use futures::sink::{Sink};
use url::{Url, ParseError};
use bytes::{Bytes, BytesMut};

pub enum PluginItem {
  Data(Bytes),
  Money(u64),
}

pub struct PluginBtp<S> {
  inner: S,
  request_id: u32,
}

impl<S> Stream for PluginBtp<S>
where
  S: Stream,
  S::Item: Into<Vec<u8>>,
  S::Error: StdError,
{
  type Item = PluginItem;
  type Error = ();

  fn poll(&mut self) -> Result<Async<Option<PluginItem>>, Self::Error> {
    match self.inner.poll() {
      Ok(Async::Ready(Some(message))) => {
        let packet = deserialize_packet(&message.into()).unwrap();
        match packet {
          BtpPacket::Message(message) => {
            for protocol_data in message.protocol_data.into_iter() {
              match protocol_data.protocol_name.as_ref() {
                "ilp" => {
                  return Ok(Async::Ready(Some(PluginItem::Data(Bytes::from(protocol_data.data)))))
                },
                _ => return Err(())
              }
            }
            return Err(())
          },
          BtpPacket::Response(response) => {
            // TODO how do we match up requests and responses?
            // should that happen in this stream thing or in something that wraps it?

          },
          _ => return Err(())
        }
        // TODO parse the websocket message
        // println!("Got response {:?}", message);
        // Ok(Async::Ready(Some(PluginItem::Money(1))))
      },
      Ok(Async::Ready(None)) => Ok(Async::Ready(None)),
      Ok(Async::NotReady) => Ok(Async::NotReady),
      Err(e) => {
        // println!("Error: {}", e);
        Err(())
      }
    }
  }
}

impl<S> Sink for PluginBtp<S>
where
  S: Sink,
  S::SinkItem: From<Vec<u8>>,
  S::SinkError: StdError,
{
  type SinkItem = PluginItem;
  type SinkError = ();

  fn start_send(&mut self, item: PluginItem) -> Result<AsyncSink<Self::SinkItem>, Self::SinkError> {
    match item {
      PluginItem::Data(data) => {
        let btp = BtpMessage {
          request_id: {
            self.request_id += 1;
            self.request_id
          },
          protocol_data: vec![ProtocolData {
            protocol_name: String::from("ilp"),
            content_type: ContentType::ApplicationOctetStream,
            data: data.to_vec()
          }]
        };
        let serialized = btp.to_bytes().unwrap();
        self.inner.start_send(serialized.into())
          .map(|result| {
            match result {
              AsyncSink::Ready => AsyncSink::Ready,
              AsyncSink::NotReady(_) => AsyncSink::NotReady(PluginItem::Data(data))
            }
          })
          .map_err(|err| {
            println!("Error sending {}", err);
          })
      },
      PluginItem::Money(amount) => {
        Err(())
      }
    }
  }

  fn poll_complete(&mut self) -> Result<Async<()>, Self::SinkError> {
    self.inner.poll_complete().map_err(|e| {
      println!("Polling error {}", e);
    })
  }
}

pub fn connect_async(server: &str) -> Box<Future<Item = PluginBtp<WebSocketStream<MaybeTlsStream<TcpStream>>>, Error = ()> + 'static + Send> {
  let server = Url::parse(server).unwrap();
  let future = connect_websocket(server.clone()).and_then(move |(ws, _ )| {
    println!("Connected to ${}", &server);
    Ok(PluginBtp {
      inner: ws,
      request_id: 0,
    })
  }).map_err(|e| {
    println!("Error connecting: {}", e);
  });
  Box::new(future)
}
