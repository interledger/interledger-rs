extern crate ilp;
extern crate tokio;

use tokio::prelude::*;
use ilp::plugin_btp::{PluginBtp, Plugin};

fn main() {
  let plugin = PluginBtp::new("ws://localhost:7768").unwrap();
  let connect = plugin.connect().map_err(|e| {
    println!("Error: {}", e);
  });
  tokio::runtime::run(connect);
}
