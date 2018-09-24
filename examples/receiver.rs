extern crate ilp;
extern crate tokio;
extern crate bytes;
extern crate futures;
extern crate ring;
extern crate chrono;
extern crate env_logger;

use tokio::prelude::*;
use ilp::plugin::btp::{connect_async, ClientPlugin};
use ilp::ilp::{IlpPacket, IlpFulfill};
use ilp::stream::Connection;
use ilp::spsp::listen_with_random_secret;
use std::sync::{Arc,Mutex};
use futures::{Stream};

fn main() {
  env_logger::init();

  let future = connect_async("ws://bob:bob@localhost:7768")
  .and_then(move |plugin: ClientPlugin| {
    println!("Conected receiver");

    listen_with_random_secret(plugin, 3000)
      .and_then(|listener| {
        listener.for_each(|conn: Connection| {
          println!("Got incoming connection");
          conn.for_each(|stream| {
            println!("Got incoming stream");
            stream.for_each(|amount| {
              println!("Got incoming money {}", amount);
              Ok(())
            })
          })
        })
        .map_err(|err| {
          println!("Error in listener {:?}", err);
        })
        .map(|_| ())
      })
  });

  tokio::runtime::run(future);
}
