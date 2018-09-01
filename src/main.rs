extern crate ilp;
extern crate tokio;
extern crate bytes;

use tokio::prelude::*;
use ilp::plugin_btp::{connect_async, PluginBtp, Plugin};
use bytes::Bytes;


fn main() {
  let future = connect_async("ws://localhost:7768")
    // .and_then(|plugin| {
    //   plugin.send_data(Bytes::from_slice(&vec![1, 2, 3]))
    // })
    // .and_then(|(response, plugin)| {
    .and_then(|plugin| {
      plugin.send_money(1234 as u64)
    })
    .and_then(|plugin| {
      Ok(())
    });
  tokio::runtime::run(future);
}
