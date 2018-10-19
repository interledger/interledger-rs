extern crate ilp;
extern crate tokio;
extern crate bytes;
#[macro_use]
extern crate futures;
extern crate ring;
extern crate chrono;
extern crate env_logger;
extern crate tokio_io;

use tokio::prelude::*;
use ilp::plugin::btp::connect_async;
use ilp::spsp::listen_with_random_secret;
use futures::{Stream, Future};
use tokio_io::AsyncRead;
use futures::future::poll_fn;

fn main() {
  env_logger::init();

  let future = connect_async("ws://bob:bob@localhost:7768")
  .map_err(|err| {
    println!("{}", err);
  })
  .and_then(move |plugin| {
    println!("Conected receiver");

    listen_with_random_secret(plugin, 3000)
      .map_err(|err| {
        println!("Error listening {:?}", err);
      })
      .and_then(|listener| {
        listener.for_each(|(id, conn)| {
          println!("Got incoming connection {}", id);
          let handle_connection = conn.for_each(|mut stream| {
            let stream_id = stream.id;
            println!("Got incoming stream {}", &stream_id);
            let handle_money = stream.money.clone().for_each(|amount| {
              println!("Got incoming money {}", amount);
              Ok(())
            })
            .and_then(move |_| {
              println!("Money stream {} closed", stream_id);
              Ok(())
            });
            tokio::spawn(handle_money);

            // TODO fix inconsistent data receiving
            let handle_data = poll_fn(move || {
              let mut data: [u8; 100] = [0; 100];
              try_ready!(stream.data.poll_read(&mut data[..])
                .map_err(|err| {
                  println!("Error polling stream for data {:?}", err);
                }));
              println!("Got incoming data: {}", String::from_utf8(Vec::from(&data[..])).unwrap());
              Ok(Async::Ready(()))
            });
            tokio::spawn(handle_data);
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
