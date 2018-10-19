extern crate ilp;
#[macro_use]
extern crate clap;
extern crate futures;
extern crate tokio;

use clap::{App, Arg, SubCommand};
use futures::{Future, Stream};
use std::sync::Arc;

pub fn main() {
  let moneyd_url = format!("btp+ws://{}:{}@localhost:7768", ilp::plugin::btp::random_token(), ilp::plugin::btp::random_token());
  let mut app = App::new("ilp")
    .about("Blazing fast Interledger CLI written in Rust")
    .subcommand(
      SubCommand::with_name("spsp")
        .about("Client and Server for the Simple Payment Setup Protocol (SPSP)")
        .subcommands(vec![
          SubCommand::with_name("server")
            .about("Run an SPSP Server that automatically accepts incoming money")
            .args(&[
              Arg::with_name("port")
                .long("port")
                .short("p")
                .takes_value(true)
                .default_value("3000"),
              Arg::with_name("btp_server")
                .long("btp_server")
                .default_value(&moneyd_url),
            ]),
          SubCommand::with_name("pay")
            .about("Send an SPSP payment")
            .args(&[
              Arg::with_name("receiver")
                .long("receiver")
                .short("r")
                .takes_value(true)
                .required(true),
              Arg::with_name("amount")
                .long("amount")
                .short("a")
                .takes_value(true)
                .required(true),
              Arg::with_name("btp_server")
                .long("btp_server")
                .default_value(&moneyd_url),
            ]),
        ]),
    );

  match app.clone().get_matches().subcommand() {
    ("spsp", Some(matches)) => match matches.subcommand() {
      ("server", Some(matches)) => {
        let btp_server = value_t!(matches, "btp_server", String).expect("BTP Server URL is required");
        let port = value_t!(matches, "port", u16).expect("Invalid port");
        run_spsp_server(btp_server, port);
      }
      ("pay", Some(matches)) => {
        let btp_server = value_t!(matches, "btp_server", String).expect("BTP Server URL is required");
        let receiver = value_t!(matches, "receiver", String).expect("Receiver is required");
        let amount = value_t!(matches, "amount", u64).expect("Invalid amount");
        send_spsp_payment(btp_server, receiver, amount);
      }
      _ => app.print_help().unwrap(),
    },
    _ => app.print_help().unwrap(),
  }
}

fn send_spsp_payment(btp_server: String, receiver: String, amount: u64) {
  let run = ilp::plugin::btp::connect_async(&btp_server)
    .map_err(|err| {
      println!("Error connecting to BTP server: {:?}", err);
    })
    .and_then(move |plugin| {
      ilp::spsp::pay(plugin, &receiver, amount.clone())
        .map_err(|err| {
          println!("Error sending SPSP payment: {:?}", err);
        })
        .and_then(move |delivered| {
          println!(
            "Sent: {}, delivered: {} (in the receiver's units)",
            amount, delivered
          );
          Ok(())
        })
    });
  tokio::run(run);
}

fn run_spsp_server(btp_server: String, port: u16) {
  let run = ilp::plugin::btp::connect_async(&btp_server)
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
}
