extern crate interledger;
#[macro_use]
extern crate clap;

use base64;
use clap::{App, Arg, SubCommand};
use interledger::send_spsp_payment;
use ring::rand::{SecureRandom, SystemRandom};

fn random_token() -> String {
    // TODO should this be shorter?
    let mut bytes: [u8; 32] = [0; 32];
    SystemRandom::new().fill(&mut bytes).unwrap();
    base64::encode_config(&bytes, base64::URL_SAFE_NO_PAD)
}

pub fn main() {
    env_logger::init();

    let moneyd_url = format!(
        "btp+ws://{}:{}@localhost:7768",
        random_token(),
        random_token()
    );
    let mut app = App::new("interledger")
        .about("Blazing fast Interledger CLI written in Rust")
        .subcommand(
            SubCommand::with_name("spsp")
                .about("Client and Server for the Simple Payment Setup Protocol (SPSP)")
                .subcommands(vec![
                    //   SubCommand::with_name("server")
                    //     .about("Run an SPSP Server that automatically accepts incoming money")
                    //     .args(&[
                    //       Arg::with_name("port")
                    //         .long("port")
                    //         .short("p")
                    //         .takes_value(true)
                    //         .default_value("3000")
                    //         .help("Port that the server should listen on"),
                    //       Arg::with_name("btp_server")
                    //         .long("btp_server")
                    //         .default_value(&moneyd_url)
                    //         .help("URI of a moneyd or BTP Server to listen on"),
                    //       Arg::with_name("notification_endpoint")
                    //         .long("notification_endpoint")
                    //         .takes_value(true)
                    //         .help("URL where notifications of incoming money will be sent (via HTTP POST)"),
                    //       Arg::with_name("quiet")
                    //         .long("quiet")
                    //         .help("Suppress log output"),
                    //     ]),
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
            // ("server", Some(matches)) => {
            //     let btp_server =
            //         value_t!(matches, "btp_server", String).expect("BTP Server URL is required");
            //     let port = value_t!(matches, "port", u16).expect("Invalid port");
            //     let notification_endpoint = value_t!(matches, "notification_endpoint", String).ok();
            //     let quiet = matches.is_present("quiet");
            //     run_spsp_server(&btp_server, port, notification_endpoint, quiet);
            // }
            ("pay", Some(matches)) => {
                let btp_server =
                    value_t!(matches, "btp_server", String).expect("BTP Server URL is required");
                let receiver = value_t!(matches, "receiver", String).expect("Receiver is required");
                let amount = value_t!(matches, "amount", u64).expect("Invalid amount");
                let quiet = matches.is_present("quiet");
                send_spsp_payment(&btp_server, &receiver, amount, quiet);
            }
            _ => app.print_help().unwrap(),
        },
        _ => app.print_help().unwrap(),
    }
}
