extern crate bytes;
extern crate chrono;
extern crate env_logger;
extern crate futures;
extern crate ilp;
extern crate ring;
extern crate rustls;
extern crate tokio;
extern crate webpki;

use ilp::plugin::btp::{connect_async, ClientPlugin};
use ilp::tls::connect_async as connect_tls;
use tokio::prelude::*;

fn main() {
  env_logger::init();

  let future =
    connect_async("ws://alice:alice@localhost:7768").and_then(move |plugin: ClientPlugin| {
      println!("Connected plugin");
      connect_tls(
        plugin,
        "cHXxnYouU7vLVMAbtL1xVql18kRTYAHeaMbnO2DvhO4#private.moneyd.local.bob",
      )
      .and_then(|conn| {
        println!("Connected to receiver over TLS+STREAM");
        let stream = conn.create_stream();
        stream.money.send(100).then(|_| Ok(()))
      })
    });

  tokio::runtime::run(future);
}
