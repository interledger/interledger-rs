mod client;
mod congestion;
mod connection;
mod crypto;
mod data_money_stream;
mod listener;
pub mod oneshot;
mod packet;

pub use self::client::connect_async;
pub use self::connection::Connection;
pub use self::data_money_stream::{DataMoneyStream, DataStream, MoneyStream};
pub use self::listener::{ConnectionGenerator, PrepareToSharedSecretGenerator, StreamListener};
use self::packet::*;

use futures::sync::mpsc::{unbounded, UnboundedReceiver, UnboundedSender};
use futures::Future;
use futures::{Sink, Stream};
use ilp::IlpPacket;
use plugin::{IlpRequest, Plugin};
use stream_cancel::Valved;
use tokio;

pub type StreamRequest = (u32, IlpPacket, Option<StreamPacket>);

pub fn plugin_to_channels<S>(plugin: S) -> (UnboundedSender<S::Item>, UnboundedReceiver<S::Item>)
where
    S: Plugin<Item = IlpRequest, Error = (), SinkItem = IlpRequest, SinkError = ()> + 'static,
{
    let (sink, stream) = plugin.split();
    let (outgoing_sender, outgoing_receiver) = unbounded::<IlpRequest>();
    let (incoming_sender, incoming_receiver) = unbounded::<IlpRequest>();

    // Stop reading from the plugin when the connection closes
    let (exit, stream) = Valved::new(stream);

    // Forward packets from Connection to plugin
    let receiver = outgoing_receiver.map_err(|err| {
        error!("Broken connection worker chan {:?}", err);
    });
    let forward_to_plugin = sink
        .send_all(receiver)
        .map(|_| ())
        .map_err(|err| {
            error!("Error forwarding request to plugin: {:?}", err);
        })
        .then(move |_| {
            trace!("Finished forwarding packets from Connection to plugin");
            drop(exit);
            Ok(())
        });
    tokio::spawn(forward_to_plugin);

    // Forward packets from plugin to Connection
    let handle_packets = incoming_sender
        .sink_map_err(|err| {
            error!(
                "Error forwarding packet from plugin to Connection: {:?}",
                err.into_inner()
            );
        })
        .send_all(stream)
        .then(|_| {
            trace!("Finished forwarding packets from plugin to Connection");
            Ok(())
        });
    tokio::spawn(handle_packets);

    (outgoing_sender, incoming_receiver)
}

#[derive(Fail, Debug)]
pub enum Error {
    #[fail(display = "Error connecting: {}", _0)]
    ConnectionError(String),
    #[fail(display = "Error polling: {}", _0)]
    PollError(String),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugin::mock::create_mock_plugins;
    use env_logger;
    use tokio::runtime::current_thread::Runtime;
    use tokio_io::io::{read_to_end, write_all};

    #[test]
    fn send_money_and_data() {
        env_logger::init();
        let (plugin_a, plugin_b) = create_mock_plugins();

        let run = StreamListener::bind(plugin_a, crypto::random_condition())
            .map_err(|err| panic!(err))
            .and_then(|(listener, conn_generator)| {
                let listen = listener.for_each(|(_conn_id, conn)| {
                    let handle_streams = conn.for_each(|stream| {
                        let handle_money = stream.money.clone().collect().and_then(|amounts| {
                            assert_eq!(amounts, vec![100, 500]);
                            Ok(())
                        });
                        tokio::spawn(handle_money);

                        let data: Vec<u8> = Vec::new();
                        let handle_data = read_to_end(stream.data, data)
                            .map_err(|err| panic!(err))
                            .and_then(|(_, data)| {
                                assert_eq!(data, b"here is some test data");
                                Ok(())
                            });
                        tokio::spawn(handle_data);

                        Ok(())
                    });
                    tokio::spawn(handle_streams);
                    Ok(())
                });
                tokio::spawn(listen);

                let (destination_account, shared_secret) =
                    conn_generator.generate_address_and_secret("test");
                connect_async(plugin_b, destination_account, shared_secret)
                    .map_err(|err| panic!(err))
                    .and_then(|conn| {
                        let stream = conn.create_stream();

                        stream
                            .money
                            .clone()
                            .send(100)
                            .map_err(|err| panic!(err))
                            .and_then(|_| {
                                stream
                                    .money
                                    .clone()
                                    .send(500)
                                    .map_err(|err| panic!(err))
                                    .and_then(|_| {
                                        write_all(stream.data, b"here is some test data")
                                            .map_err(|err| panic!(err))
                                            .map(|_| ())
                                    })
                            })
                            .and_then(move |_| conn.close())
                    })
            });
        let mut runtime = Runtime::new().unwrap();
        runtime.block_on(run).unwrap();
    }
}
