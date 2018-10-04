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
use std::sync::Arc;
use tokio::prelude::*;

fn main() {
  env_logger::init();

  let future =
    connect_async("ws://alice:alice@localhost:7768").and_then(move |plugin: ClientPlugin| {
      connect_tls(plugin, "private.moneyd.local.bob").and_then(
        |(shared_secret, _plugin)| {
          println!("Shared secret {:x?}", &shared_secret[..]);
          Ok(())
        },
      )
    });

  tokio::runtime::run(future);
}
