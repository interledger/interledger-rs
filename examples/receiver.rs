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
use std::sync::{Arc,Mutex};

fn main() {
  env_logger::init();

  let fulfillment: [u8; 32] = [168,200,212,121,243,105,254,213,16,207,44,228,66,202,207,252,9,169,224,39,129,45,89,83,245,123,113,195,146,39,200,231];
  // let condition: [u8; 32] = [121,203,69,48,239,26,252,52,244,82,21,241,100,236,118,173,180,61,29,142,220,139,58,106,218,127,56,181,145,93,3,244];

  let future = connect_async("ws://bob:bob@localhost:7768")
  .and_then(move |plugin: ClientPlugin| {
    println!("Conected receiver");

    let (sink, stream) = plugin.split();
    let sink = Arc::new(Mutex::new(sink));

    stream.for_each(move |packet| {
      println!("Receiver got packet: {:?}", packet.clone());

      if let (request_id, IlpPacket::Prepare(_prepare)) = packet {
        let mut sink = sink.lock().unwrap();
        let fulfill = (request_id, IlpPacket::Fulfill(IlpFulfill::new(
          fulfillment[..].to_vec(),
          &vec![] as &[u8],
        )));
        println!("Responding with fulfill: {:?}", fulfill.clone());
        sink.start_send(fulfill).unwrap();
      }
      Ok(())
    })
  });

  tokio::runtime::run(future);
}
