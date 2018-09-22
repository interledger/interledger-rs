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
use ilp::spsp::listen_with_random_secret;
use std::sync::{Arc,Mutex};

fn main() {
  env_logger::init();

  let future = connect_async("ws://bob:bob@localhost:7768")
  .and_then(move |plugin: ClientPlugin| {
    println!("Conected receiver");

    listen_with_random_secret(plugin, 3000)
  });

  tokio::runtime::run(future);
}
