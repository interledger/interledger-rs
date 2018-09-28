extern crate ilp;
extern crate tokio;
extern crate bytes;
extern crate futures;
extern crate ring;
extern crate chrono;
extern crate env_logger;

use tokio::prelude::*;
use ilp::plugin::btp::connect_to_moneyd;
// use ilp::spsp::pay;
use ilp::spsp::connect_async;


fn main() {
  env_logger::init();

  let future = connect_to_moneyd()
    .and_then(move |plugin| {
      println!("Conected sender");

      // pay(plugin, "http://localhost:3000", 100)
      //   .and_then(|amount_sent| {
      //     println!("Sent {}", amount_sent);
      //     Ok(())
      //   })

      connect_async(plugin, "http://localhost:3000")
        .and_then(|connection| {
          let stream = connection.create_stream();
          stream.money.clone().send(100)
            .and_then(move |_| {
              stream.data.clone().write(b"hey there")
                .map_err(|err| {
                  println!("Error writing {}", err);
                })
            })
        })
    })
    .then(|_| Ok(()));

  tokio::runtime::run(future);
}
