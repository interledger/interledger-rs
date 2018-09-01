extern crate ilp;
extern crate tokio;
extern crate bytes;
extern crate futures;

use tokio::prelude::*;
use ilp::plugin_btp::{connect_async, PluginBtp, PluginItem};
use bytes::Bytes;
use futures::{Stream, Sink};


fn main() {
  let future = connect_async("ws://localhost:7768")
  .and_then(|plugin| {
    plugin.send(PluginItem::Data(Bytes::from(vec![0, 1, 2])))
    .map(|_| { () })
  });
  tokio::runtime::run(future);
}
