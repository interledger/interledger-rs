extern crate ilp;
extern crate tokio;
extern crate bytes;
extern crate futures;

use tokio::prelude::*;
use ilp::plugin_btp::{connect_async, PluginBtp};
use bytes::Bytes;
use futures::{Stream, Sink};
use ilp::IlpOrBtpPacket;
use ilp::ilp_packet_stream::IlpPacketStream;
use ilp::ilp_fulfillment_checker::IlpFulfillmentChecker;


fn main() {
  let future = connect_async("ws://localhost:7768")
  .and_then(|plugin| {
    Ok(IlpFulfillmentChecker::new(IlpPacketStream::new(plugin)))
  })
  .and_then(|stream| {
    Ok(())
  });

  tokio::runtime::run(future);
}
