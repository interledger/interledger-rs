use std::error::Error as StdError;
use btp_packet::{BtpMessage, BtpResponse, Serializable};
use tokio_tungstenite::{connect_async, WebSocketStream, MaybeTlsStream};
use tungstenite::{Error as WebSocketError};
use tokio_tcp::TcpStream;
use futures::Future;
use futures::future::{ok};
use futures::stream::{Stream, SplitSink, SplitStream};
use url::{Url, ParseError};

pub trait Plugin {
  type Error: Into<Box<StdError + Send + Sync>>;
  // TODO is this function signature right?
  fn connect<'a>(&'a mut self) -> Box<Future<Item = (), Error = Self::Error> + 'a + Send>;
  fn is_connected(&self) -> bool;
}

pub struct PluginBtp {
  server: Url,
  connected: bool,
  ws_sink: Option<SplitSink<WebSocketStream<MaybeTlsStream<TcpStream>>>>,
  ws_stream: Option<SplitStream<WebSocketStream<MaybeTlsStream<TcpStream>>>>,
}

impl PluginBtp {
  pub fn new (server: &str) -> Result<PluginBtp, ParseError> {
    Ok(PluginBtp {
      server: Url::parse(server)?,
      connected: false,
      ws_sink: None,
      ws_stream: None,
    })
  }
}

impl Plugin for PluginBtp {
  type Error = WebSocketError;

  fn connect<'a>(&'a mut self) -> Box<Future<Item = (), Error = Self::Error> + 'a + Send> {
    let future = connect_async(self.server.clone()).and_then(move |(ws_stream, _ )| {
      println!("Connected to ${}", self.server);
      let (ws_sink, ws_stream) = ws_stream.split();
      self.ws_sink = Some(ws_sink);
      self.ws_stream = Some(ws_stream);
      ok(())
    });
    Box::new(future)
  }

  fn is_connected(&self) -> bool {
    self.connected
  }
}
