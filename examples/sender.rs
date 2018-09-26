extern crate ilp;
extern crate tokio;
extern crate bytes;
extern crate futures;
extern crate ring;
extern crate chrono;
extern crate env_logger;

use tokio::prelude::*;
use ilp::plugin::btp::connect_to_moneyd;
use ilp::spsp::pay;


fn main() {
  env_logger::init();

  let future = connect_to_moneyd()
    .and_then(move |plugin| {
      println!("Conected sender");

      pay(plugin, "http://localhost:3000", 100)
        .and_then(|amount_sent| {
          println!("Sent {}", amount_sent);
          Ok(())
        })
    })
    .then(|_| Ok(()));

  tokio::runtime::run(future);
}
