mod connection;
mod crypto;
mod packet;
mod packet_stream;

pub use self::connection::Connection;
use self::packet::*;
use self::packet_stream::StreamPacketStream;

use bytes::Bytes;
use futures::Future;
use ildcp;
use plugin::{IlpRequest, Plugin};

pub fn connect_async<S, T, U>(
  plugin: S,
  destination_account: T,
  shared_secret: U,
) -> impl Future<Item = Connection<impl Plugin>, Error = ()>
where
  S: Plugin<Item = IlpRequest, Error = (), SinkItem = IlpRequest, SinkError = ()>,
  String: From<T>,
  Bytes: From<U>,
{
  ildcp::get_config(plugin)
    .map_err(|(_err, _plugin)| {
      error!("Error getting ILDCP config info");
    })
    .and_then(move |(config, plugin)| {
      let client_address: String = config.client_address;
      let conn = Connection::new(
        StreamPacketStream::new(Bytes::from(shared_secret), plugin),
        client_address,
        String::from(destination_account),
        false,
      );
      debug!("Created connection");
      Ok(conn)
    })
}
