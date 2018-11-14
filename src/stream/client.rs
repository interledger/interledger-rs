use super::Error;
use super::{plugin_to_channels, Connection};
use bytes::Bytes;
use futures::Future;
use ildcp;
use plugin::{IlpRequest, Plugin};

pub fn connect_async<S, T, U>(
    plugin: S,
    destination_account: T,
    shared_secret: U,
) -> impl Future<Item = Connection, Error = Error>
where
    S: Plugin<Item = IlpRequest, Error = (), SinkItem = IlpRequest, SinkError = ()> + 'static,
    String: From<T>,
    Bytes: From<U>,
{
    ildcp::get_config(plugin)
        .map_err(|err| Error::ConnectionError(format!("Error connecting: {}", err)))
        .and_then(move |(config, plugin)| {
            let client_address: String = config.client_address;

            let (outgoing_sender, incoming_receiver) = plugin_to_channels(plugin);
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
