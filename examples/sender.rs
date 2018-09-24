extern crate ilp;
extern crate tokio;
extern crate bytes;
extern crate futures;
extern crate ring;
extern crate chrono;
extern crate env_logger;

use tokio::prelude::*;
use ilp::plugin::btp::{connect_async, ClientPlugin};
use ilp::stream::Connection;
use ilp::spsp::{connect_async as connect_spsp};


fn main() {
  env_logger::init();

  let spsp_server = "http://localhost:3000";

  let future = connect_async("ws://alice:alice@localhost:7768")
    .and_then(move |plugin: ClientPlugin| {
      println!("Conected sender");

      connect_spsp(plugin, spsp_server)
    })
    .and_then(|mut conn: Connection| {
      let stream = conn.create_stream();
      // TODO should send_money return the stream so we don't need to clone it?
      stream.clone().send(100)
        .and_then(move |_| {
          println!("Sent money on stream. Total sent: {}, total delivered: {}", stream.total_sent(), stream.total_delivered());
          Ok(())
        })
        .map_err(|err| {
          println!("Error sending: {:?}", err);
        })
        // .and_then(move |_| {
        //   conn.close()
        // })
        .and_then(|_| {
          println!("Closed connection");
          Ok(())
        })
    })
    .then(|_| Ok(()));

  tokio::runtime::run(future);
}
