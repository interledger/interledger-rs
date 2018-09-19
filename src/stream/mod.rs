mod connection;
mod crypto;
mod packet;

pub use self::connection::Connection;
use self::packet::*;

use bytes::Bytes;
use futures::Future;
use ildcp;
use plugin::{IlpRequest, Plugin};
use ilp::IlpPacket;
use tokio;
use futures::{Stream, Sink};
use futures::sync::mpsc;

pub type StreamRequest = (u32, IlpPacket, Option<StreamPacket>);

pub fn connect_async<S, T, U>(
  plugin: S,
  destination_account: T,
  shared_secret: U,
) -> impl Future<Item = Connection, Error = ()>
where
  S: Plugin<Item = IlpRequest, Error = (), SinkItem = IlpRequest, SinkError = ()> + 'static,
  String: From<T>,
  Bytes: From<U>,
{
  ildcp::get_config(plugin)
    .map_err(|(_err, _plugin)| {
      error!("Error getting ILDCP config info");
    })
    .and_then(move |(config, plugin)| {
      let client_address: String = config.client_address;

      let (sink, stream) = plugin.split();
      let (outgoing_sender, outgoing_receiver) = mpsc::unbounded::<IlpRequest>();
      let (incoming_sender, incoming_receiver) = mpsc::unbounded::<IlpRequest>();

      // Forward packets from Connection to plugin
      let receiver = outgoing_receiver.map_err(|err| {
        error!("Broken connection worker chan {:?}", err);
      });
      let forward_to_plugin = sink
        .send_all(receiver)
        .map(|_| ())
        .map_err(|err| {
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
        })
        .map_err(|err| {
          error!("Error handling incoming packet: {:?}", err);
        });
      tokio::spawn(handle_packets);

      let conn = Connection::new(
        outgoing_sender,
        incoming_receiver,
        Bytes::from(shared_secret),
        client_address,
        String::from(destination_account),
        false,
      );

      Ok(conn)
    })
}
