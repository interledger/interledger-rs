extern crate ilp;
#[macro_use]
extern crate clap;
extern crate futures;
extern crate tokio;
extern crate hyper;
#[macro_use]
extern crate serde_json;

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
                .default_value("3000")
                .help("Port that the server should listen on"),
              Arg::with_name("btp_server")
                .long("btp_server")
                .default_value(&moneyd_url)
                .help("URI of a moneyd or BTP Server to listen on"),
              Arg::with_name("notification_endpoint")
                .long("notification_endpoint")
                .takes_value(true)
                .help("URL where notifications of incoming money will be sent (via HTTP POST)"),
            ]),
          SubCommand::with_name("pay")
            .about("Send an SPSP payment")
            .args(&[
              Arg::with_name("receiver")
                .long("receiver")
                .short("r")
                .takes_value(true)
                .required(true)
                .help("Payment Pointer of the receiver"),
              Arg::with_name("amount")
                .long("amount")
                .short("a")
                .takes_value(true)
                .required(true)
                .help("Amount to send, denominated in the BTP Server's units"),
              Arg::with_name("btp_server")
                .long("btp_server")
                .default_value(&moneyd_url)
                .help("URI of a moneyd or BTP Server to pay from"),
            ]),
        ]),
    );

  match app.clone().get_matches().subcommand() {
    ("spsp", Some(matches)) => match matches.subcommand() {
      ("server", Some(matches)) => {
        let btp_server = value_t!(matches, "btp_server", String).expect("BTP Server URL is required");
        let port = value_t!(matches, "port", u16).expect("Invalid port");
        let notification_endpoint = value_t!(matches, "notification_endpoint", String).ok();
        run_spsp_server(&btp_server, port, notification_endpoint);
      }
      ("pay", Some(matches)) => {
        let btp_server = value_t!(matches, "btp_server", String).expect("BTP Server URL is required");
        let receiver = value_t!(matches, "receiver", String).expect("Receiver is required");
        let amount = value_t!(matches, "amount", u64).expect("Invalid amount");
        send_spsp_payment(&btp_server, receiver, amount);
      }
      _ => app.print_help().unwrap(),
    },
    _ => app.print_help().unwrap(),
  }
}

fn send_spsp_payment(btp_server: &str, receiver: String, amount: u64) {
  let run = ilp::plugin::btp::connect_async(&btp_server)
    .map_err(|err| {
      println!("Error connecting to BTP server: {:?}", err);
    })
    .and_then(move |plugin| {
      ilp::spsp::pay(plugin, &receiver, amount)
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

fn run_spsp_server(btp_server: &str, port: u16, notification_endpoint: Option<String>) {
  let notification_endpoint = Arc::new(notification_endpoint);
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
          let handle_connections = listener.for_each(move |(id, connection)| {
            // TODO should the STREAM or SPSP server automatically remove this?
            let split: Vec<&str> = id.splitn(2, '~').collect();
            let conn_id = Arc::new(split[1].to_string());

            let notification_endpoint = Arc::clone(&notification_endpoint);
            // TODO close the connection if it doesn't have a tag?
            let handle_streams = connection.for_each(move |stream| {
              let conn_id = Arc::clone(&conn_id);
              let notification_endpoint = Arc::clone(&notification_endpoint);

              let handle_money = stream.money.for_each(move |amount| {
                if let Some(ref url) = *notification_endpoint {
                  let body = json!({
                    "receiver": *conn_id,
                    "amount": amount,
                  }).to_string();
                  let req = hyper::Request::post(url)
                    .header("Content-Type", "application/json")
                    .body(hyper::Body::from(body))
                    .unwrap();
                  let conn_id = Arc::clone(&conn_id);
                  let send_notification = hyper::Client::new()
                    .request(req)
                    .map_err(move |err| {
                      println!("Error sending notification (got incoming money: {} for receiver: {}): {:?}", amount, conn_id, err);
                    })
                    .and_then(|_| {
                      Ok(())
                    });
                  tokio::spawn(send_notification);
                } else {
                  println!("Got incoming money: {} for connection: {}", amount, conn_id);
                }

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
