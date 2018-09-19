extern crate ilp;
extern crate tokio;
extern crate bytes;
extern crate futures;
extern crate ring;
extern crate chrono;
extern crate env_logger;

use tokio::prelude::*;
use ilp::plugin::btp::{connect_async, ClientPlugin};
use ilp::ilp::{IlpPacket, IlpPrepare};
use chrono::{Utc, Duration};
use bytes::Bytes;
use ilp::stream::{connect_async as connect_stream};
use ilp::spsp::{connect_async as connect_spsp};


fn main() {
  env_logger::init();

  let spsp_server = "http://localhost:3000";

  let future = connect_async("ws://alice:alice@localhost:7768")
    .and_then(move |plugin: ClientPlugin| {
      println!("Conected sender");

      connect_spsp(plugin, spsp_server)
    })
    .then(|_| Ok(()));

  tokio::runtime::run(future);
}
