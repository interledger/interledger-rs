use chrono::Utc;
use clap::{App, Arg, SubCommand};
use futures::{Future, Stream};
use plugin::btp;
use spsp;
use std::sync::Arc;

pub fn run(app_name: &str) {
    env_logger::init();

    let moneyd_url = format!(
        "btp+ws://{}:{}@localhost:7768",
        btp::random_token(),
        btp::random_token()
    );
    let mut app = App::new(app_name)
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
              Arg::with_name("quiet")
                .long("quiet")
                .help("Suppress log output"),
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
              Arg::with_name("quiet")
                .long("quiet")
                .help("Suppress log output"),
            ]),
        ]),
        );

    match app.clone().get_matches().subcommand() {
        ("spsp", Some(matches)) => match matches.subcommand() {
            ("server", Some(matches)) => {
                let btp_server =
                    value_t!(matches, "btp_server", String).expect("BTP Server URL is required");
                let port = value_t!(matches, "port", u16).expect("Invalid port");
                let notification_endpoint = value_t!(matches, "notification_endpoint", String).ok();
                let quiet = matches.is_present("quiet");
                run_spsp_server(&btp_server, port, notification_endpoint, quiet);
            }
            ("pay", Some(matches)) => {
                let btp_server =
                    value_t!(matches, "btp_server", String).expect("BTP Server URL is required");
                let receiver = value_t!(matches, "receiver", String).expect("Receiver is required");
                let amount = value_t!(matches, "amount", u64).expect("Invalid amount");
                let quiet = matches.is_present("quiet");
                send_spsp_payment(&btp_server, receiver, amount, quiet);
            }
            _ => app.print_help().unwrap(),
        },
        _ => app.print_help().unwrap(),
    }
}

fn send_spsp_payment(btp_server: &str, receiver: String, amount: u64, quiet: bool) {
    let run = btp::connect_async(&btp_server)
        .map_err(|err| {
            eprintln!("Error connecting to BTP server: {:?}", err);
            eprintln!("(Hint: is moneyd running?)");
        })
        .and_then(move |plugin| {
            spsp::pay(plugin, &receiver, amount)
                .map_err(|err| {
                    eprintln!("Error sending SPSP payment: {:?}", err);
                })
                .and_then(move |delivered| {
                    if !quiet {
                        println!(
                            "Sent: {}, delivered: {} (in the receiver's units)",
                            amount, delivered
                        );
                    }
                    Ok(())
                })
        });
    tokio::run(run);
}

fn run_spsp_server(
    btp_server: &str,
    port: u16,
    notification_endpoint: Option<String>,
    quiet: bool,
) {
    let notification_endpoint = Arc::new(notification_endpoint);

    // TODO make sure that the client keeps the connections alive
    let client = Arc::new(reqwest::async::Client::new());

    let run = btp::connect_async(&btp_server)
    .map_err(|err| {
      eprintln!("Error connecting to BTP server: {:?}", err);
      eprintln!("(Hint: is moneyd running?)");
    })
    .and_then(move |plugin| {
      spsp::listen_with_random_secret(plugin, port)
        .map_err(|err| {
          eprintln!("Error listening: {}", err);
        })
        .and_then(move |listener| {
          let handle_connections = listener.for_each(move |(id, connection)| {
            // TODO should the STREAM or SPSP server automatically remove this?
            let split: Vec<&str> = id.splitn(2, '~').collect();

            let client = Arc::clone(&client);
            // TODO close the connection if it doesn't have a tag?
            let conn_id = Arc::new(split[1].to_string());
            let conn_id_clone = Arc::clone(&conn_id);

            let notification_endpoint = Arc::clone(&notification_endpoint);
            let handle_streams = connection.for_each(move |stream| {
              let notification_endpoint = Arc::clone(&notification_endpoint);
              let client = Arc::clone(&client);
              let conn_id = Arc::clone(&conn_id);

              let handle_money = stream.money.for_each(move |amount| {
                if let Some(ref url) = *notification_endpoint {
                  let conn_id = Arc::clone(&conn_id);
                  let body = json!({
                    "receiver": *conn_id,
                    "amount": amount,
                  }).to_string();
                  let send_notification = client.post(url)
                    .header("Content-Type", "application/json")
                    .body(body)
                    .send()
                    .map_err(move |err| {
                      eprintln!("Error sending notification (got incoming money: {} for receiver: {}): {:?}", amount, conn_id, err);
                    })
                    .and_then(|_| {
                      Ok(())
                    });
                  tokio::spawn(send_notification);
                } else if !quiet {
                  println!("{}: Received: {} for connection: {}", Utc::now().to_rfc3339(), amount, conn_id);
                }

                Ok(())
              });
              tokio::spawn(handle_money);
              Ok(())
            })
            .and_then(move |_| {
              if !quiet {
                println!("{}: Connection closed: {}", Utc::now().to_rfc3339(), conn_id_clone);
              }
              Ok(())
            });
            tokio::spawn(handle_streams);
            Ok(())
          });
          tokio::spawn(handle_connections);
          if !quiet {
            println!("Listening for SPSP connections on port: {}", port);
          }
          Ok(())
        })
    });
    tokio::run(run);
}
