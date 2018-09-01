use std::error::Error as StdError;
use btp_packet::{BtpMessage, BtpResponse, Serializable, ProtocolData, ContentType};
use tokio_tungstenite::{connect_async as connect_websocket, WebSocketStream, MaybeTlsStream};
use tungstenite::{Error as WebSocketError, Message as WebSocketMessage};
use tokio_tcp::TcpStream;
use futures::{Future, Async, AsyncSink};
use futures::future::{ok};
use futures::stream::{Stream, SplitSink, SplitStream};
use futures::sink::{Sink};
use url::{Url, ParseError};
use bytes::{Bytes, BytesMut};

pub enum PluginItem {
  Data(Bytes),
  Money(u64),
}

pub struct PluginBtp {
  server: Url,
  ws: WebSocketStream<MaybeTlsStream<TcpStream>>,
  request_id: u32,
}

impl Stream for PluginBtp {
  type Item = PluginItem;
  type Error = Box<StdError>;

  fn poll(&mut self) -> Result<Async<Option<Self::Item>>, Self::Error> {
    match self.ws.poll() {
      Ok(Async::Ready(Some(ws_message))) => {
        // TODO parse the websocket message
        println!("Got response {:?}", ws_message);
        Ok(Async::Ready(Some(PluginItem::Money(1))))
      },
      Ok(Async::Ready(None)) => Ok(Async::Ready(None)),
      Ok(Async::NotReady) => Ok(Async::NotReady),
      Err(e) => Err(Box::from(e))
    }
  }
}

impl Sink for PluginBtp {
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
        let ws_message = WebSocketMessage::Binary(btp.to_bytes().unwrap());
        self.ws.start_send(ws_message)
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
    self.ws.poll_complete().map_err(|e| {
      println!("Polling error {}", e);
    })
  }
}

pub fn connect_async(server: &str) -> Box<Future<Item = PluginBtp, Error = ()> + 'static + Send> {
  let server = Url::parse(server).unwrap();
  let future = connect_websocket(server.clone()).and_then(move |(ws, _ )| {
    println!("Connected to ${}", &server);
    Ok(PluginBtp {
      server,
      ws,
      request_id: 0,
    })
  }).map_err(|e| {
    println!("Error connecting: {}", e);
  });
  Box::new(future)
}
