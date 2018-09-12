extern crate ilp;
extern crate tokio;
extern crate bytes;
extern crate futures;
extern crate ring;
extern crate chrono;
extern crate env_logger;

use tokio::prelude::*;
use ilp::btp_packet_stream::{connect_async, BtpPacketStream};
use bytes::Bytes;
use futures::{Stream, Sink};
use ilp::IlpOrBtpPacket;
use ilp::ilp_packet_stream::IlpPacketStream;
use ilp::ilp_packet::{IlpPacket, IlpPrepare, IlpFulfill, IlpReject};
use ilp::ilp_fulfillment_checker::IlpFulfillmentChecker;
use ilp::btp_request_id_checker::BtpRequestIdCheckerStream;
use chrono::{DateTime, Utc, Duration};
use std::sync::{Arc,Mutex};

fn main() {
  env_logger::init();

  // let fulfillment: [u8; 32] = [168,200,212,121,243,105,254,213,16,207,44,228,66,202,207,252,9,169,224,39,129,45,89,83,245,123,113,195,146,39,200,231];
  let condition: [u8; 32] = [
    121, 203, 69, 48, 239, 26, 252, 52, 244, 82, 21, 241, 100, 236, 118, 173, 180, 61, 29, 142,
    220, 139, 58, 106, 218, 127, 56, 181, 145, 93, 3, 244,
  ];

  let future = connect_async("ws://alice:alice@localhost:7768")
    .and_then(|stream| {
      Ok(IlpFulfillmentChecker::new(IlpPacketStream::new(
        BtpRequestIdCheckerStream::new(stream),
      )))
    })
    .and_then(move |plugin| {
      println!("Conected sender");

      let (sink, stream) = plugin.split();

      let prepare = IlpOrBtpPacket::Ilp(
        99,
        IlpPacket::Prepare(IlpPrepare::new(
          String::from("private.moneyd.local.bob"),
          100,
          Bytes::from(&condition[..]),
          Utc::now() + Duration::seconds(30),
          Bytes::new(),
        )),
      );

      println!("Sending packet: {:?}", prepare.clone());
      sink.send(prepare).and_then(move |_| {
      // sink.start_send(prepare);
        stream.for_each(|packet| {
          println!("Sender got response packet {:?}", packet);

          Ok(())
        })
      })
    })
    .then(|_| Ok(()));

  tokio::runtime::run(future);
}
