extern crate ilp;
extern crate tokio;
extern crate bytes;
extern crate futures;

use tokio::prelude::*;
use ilp::btp_packet_stream::{connect_async, BtpPacketStream};
use bytes::Bytes;
use futures::{Stream, Sink};
use ilp::IlpOrBtpPacket;
use ilp::ilp_packet_stream::IlpPacketStream;
use ilp::ilp_fulfillment_checker::IlpFulfillmentChecker;
use ilp::btp_request_id_checker::BtpRequestIdCheckerStream;

fn main() {
  let future = connect_async("ws://localhost:7768")
  .and_then(|plugin| {
    Ok(IlpFulfillmentChecker::new(IlpPacketStream::new(BtpRequestIdCheckerStream::new(plugin))))
  })
  .and_then(|stream| {
    Ok(())
  });

  tokio::runtime::run(future);
}
