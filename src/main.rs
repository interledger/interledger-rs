extern crate ilp;
#[macro_use]
extern crate clap;
extern crate futures;
extern crate tokio;

use clap::{App, Arg, SubCommand};
use futures::{Future, Stream};
use std::sync::Arc;

pub fn main() {
  let mut app = App::new("ilp")
    .about("Blazing fast Interledger CLI written in Rust")
    .subcommand(
      SubCommand::with_name("spsp")
        .about("Client and Server for the Simple Payment Setup Protocol (SPSP)")
        .subcommand(
          SubCommand::with_name("server")
            .about("Run an SPSP Server that automatically accepts incoming money")
            .arg(Arg::with_name("port").short("p").default_value("3000")),
        ),
    );
  let matches = app.clone().get_matches();

  if let Some(matches) = matches.subcommand_matches("spsp") {

    // Run SPSP Server
    if let Some(matches) = matches.subcommand_matches("server") {
      let port = value_t!(matches, "port", u16).expect("Invalid port");
      let run = ilp::plugin::btp::connect_to_moneyd()
        .map_err(|err| {
          println!("Error connecting to BTP server: {:?}", err);
        })
        .and_then(move |plugin| {
          ilp::spsp::listen_with_random_secret(plugin, port)
            .map_err(|err| {
              println!("Error listening: {}", err);
            })
            .and_then(move |listener| {
              let handle_connections = listener.for_each(|(id, connection)| {
                let conn_id = Arc::new(id);
                let handle_streams = connection.for_each(move |stream| {
                  let conn_id = Arc::clone(&conn_id);
                  let handle_money = stream.money.for_each(move |amount| {
                    println!("Connection {} got incoming money: {}", conn_id, amount);
                    Ok(())
                  });
                  tokio::spawn(handle_money);
                  Ok(())
                });
                tokio::spawn(handle_streams);
                Ok(())
              });
              tokio::spawn(handle_connections);
              println!("Listening for SPSP connections on port: {}", port);
              Ok(())
            })
        });
      tokio::run(run);
      return;
    }
  }

  app.print_help().unwrap();
}
