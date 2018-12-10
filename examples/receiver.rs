extern crate bytes;
extern crate chrono;
extern crate env_logger;
extern crate futures;
extern crate interledger;
extern crate ring;
extern crate tokio;
extern crate tokio_io;

use futures::{Future, Stream};
use interledger::plugin::btp::connect_async;
use interledger::spsp::listen_with_random_secret;
use tokio_io::io::read_to_end;

fn main() {
    env_logger::init();

    let future = connect_async("ws://bob:bob@localhost:7768")
        .map_err(|err| {
            println!("{}", err);
        })
        .and_then(move |plugin| {
            println!("Connected receiver");

            listen_with_random_secret(plugin, 3000)
                .map_err(|err| {
                    println!("Error listening {:?}", err);
                })
                .and_then(|listener| {
                    listener
                        .for_each(|(id, conn)| {
                            println!("Got incoming connection {}", id);
                            let handle_connection = conn
                                .for_each(|stream| {
                                    let stream_id = stream.id;
                                    println!("Got incoming stream {}", &stream_id);
                                    let handle_money = stream
                                        .money
                                        .clone()
                                        .for_each(|amount| {
                                            println!("Got incoming money {}", amount);
                                            Ok(())
                                        })
                                        .and_then(move |_| {
                                            println!("Money stream {} closed", stream_id);
                                            Ok(())
                                        });
                                    tokio::spawn(handle_money);

                                    // TODO fix inconsistent data receiving
                                    let data: Vec<u8> = Vec::new();
                                    let handle_data = read_to_end(stream.data, data)
                                        .map_err(|err| {
                                            println!("Error reading data: {}", err);
                                        })
                                        .and_then(|(_stream, data)| {
                                            println!(
                                                "Got incoming data: {}",
                                                String::from_utf8(Vec::from(&data[..])).unwrap()
                                            );
                                            Ok(())
                                        });
                                    tokio::spawn(handle_data);
                                    Ok(())
                                })
                                .then(|_| {
                                    println!("Connection was closed");
                                    Ok(())
                                });

                            tokio::spawn(handle_connection);
                            Ok(())
                        })
                        .map_err(|err| {
                            println!("Error in listener {:?}", err);
                        })
                        .map(|_| ())
                })
        });

    tokio::runtime::run(future);
}
