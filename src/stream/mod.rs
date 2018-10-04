mod client;
mod connection;
mod crypto;
mod listener;
mod packet;
mod data_money_stream;

pub use self::client::connect_async;
pub use self::connection::Connection;
pub use self::listener::{ConnectionGenerator, StreamListener, PrepareToSharedSecretGenerator};
pub use self::data_money_stream::MoneyStream;
use self::packet::*;

use futures::sync::mpsc::{unbounded, UnboundedReceiver, UnboundedSender};
use futures::Future;
use futures::{Sink, Stream};
use ilp::IlpPacket;
use plugin::{IlpRequest, Plugin};
use tokio;

pub type StreamRequest = (u32, IlpPacket, Option<StreamPacket>);

pub fn plugin_to_channels<S>(plugin: S) -> (UnboundedSender<S::Item>, UnboundedReceiver<S::Item>)
where
  S: Plugin<Item = IlpRequest, Error = (), SinkItem = IlpRequest, SinkError = ()> + 'static,
{
  let (sink, stream) = plugin.split();
  let (outgoing_sender, outgoing_receiver) = unbounded::<IlpRequest>();
  let (incoming_sender, incoming_receiver) = unbounded::<IlpRequest>();

  // Forward packets from Connection to plugin
  let receiver = outgoing_receiver.map_err(|err| {
    error!("Broken connection worker chan {:?}", err);
  });
  let forward_to_plugin = sink.send_all(receiver).map(|_| ()).map_err(|err| {
    error!("Error forwarding request to plugin: {:?}", err);
  });
  tokio::spawn(forward_to_plugin);

  // Forward packets from plugin to Connection
  let handle_packets = incoming_sender
    .sink_map_err(|_| ())
    .send_all(stream)
    .and_then(|_| {
      debug!("Finished forwarding packets from plugin to Connection");
      Ok(())
    }).map_err(|err| {
      error!("Error handling incoming packet: {:?}", err);
    });
  tokio::spawn(handle_packets);

  (outgoing_sender, incoming_receiver)
}
