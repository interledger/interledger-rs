extern crate ilp;
extern crate tokio;

use tokio::prelude::*;
use ilp::plugin_btp::{connect_async, PluginBtp, Plugin};

fn main() {
  let future = connect_async("ws://localhost:7768")
    .and_then(|plugin| {
      plugin.send_data(&vec![1, 2, 3])
    })
    .and_then(|(response, plugin)| {
      println!("Got response {:?}", response);
      Ok(())
    });
  tokio::runtime::run(future);
}
