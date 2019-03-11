extern crate interledger;
#[macro_use]
extern crate clap;

use clap::{App, Arg, ArgGroup, SubCommand};
use interledger::*;
use interledger_ildcp::IldcpResponseBuilder;

#[allow(clippy::cyclomatic_complexity)]
pub fn main() {
    env_logger::init();

    let moneyd_uri = format!(
        "btp+ws://{}:{}@localhost:7768",
        random_token(),
        random_token()
    );
    let mut app = App::new("interledger")
        .about("Blazing fast Interledger CLI written in Rust")
        .subcommands(vec![
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
                                .default_value(&moneyd_uri)
                                .help("URI of a moneyd or BTP Server to listen on"),
                            Arg::with_name("ilp_over_http")
                                .long("use_ilp_over_http")
                                .help("Accept ILP packets sent over HTTP instead of connecting to a BTP server"),
                            Arg::with_name("ilp_address")
                                .long("ilp_address")
                                .takes_value(true)
                                .help("The server's ILP address (Required for ilp_over_http)"),
                            Arg::with_name("incoming_auth_token")
                                .long("incoming_auth_token")
                                .takes_value(true)
                                .help("Token that must be used to authenticate incoming requests (Required for ilp_over_http)"),
                            Arg::with_name("quiet")
                                .long("quiet")
                                .help("Suppress log output"),
                        ])
                        .group(ArgGroup::with_name("http_options").requires_all(&["ilp_over_http", "ilp_address", "incoming_auth_token"])),
                    SubCommand::with_name("pay")
                        .about("Send an SPSP payment")
                        .args(&[
                            Arg::with_name("btp_server")
                                .long("btp_server")
                                .takes_value(true)
                                .default_value(&moneyd_uri)
                                .help("URI of a moneyd or BTP Server to pay from"),
                            Arg::with_name("http_server")
                                .long("http_server")
                                .takes_value(true)
                                .help("HTTP URL of the connector to pay from"),
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
                                .help("Amount to send, denominated in the connector's units"),
                            Arg::with_name("quiet")
                                .long("quiet")
                                .help("Suppress log output"),
                        ]),
                ]),
                SubCommand::with_name("moneyd")
                    .about("Run a local connector")
                    .subcommand(SubCommand::with_name("local")
                        .about("Run locally without connecting to a remote connector")
                        .args(&[
                            Arg::with_name("port")
                                .long("port")
                                .default_value("7768")
                                .help("Port to listen for BTP connections on"),
                            Arg::with_name("ilp_address")
                                .long("ilp_address")
                                .default_value("private.local"),
                            Arg::with_name("asset_code")
                                .long("asset_code")
                                .default_value("XYZ"),
                            Arg::with_name("asset_scale")
                                .long("asset_scale")
                                .default_value("9"),
                        ])
                    ),
        ]);

    match app.clone().get_matches().subcommand() {
        ("spsp", Some(matches)) => match matches.subcommand() {
            ("server", Some(matches)) => {
                let port = value_t!(matches, "port", u16).expect("Invalid port");
                let quiet = matches.is_present("quiet");
                if matches.is_present("ilp_over_http") {
                    let client_address =
                        value_t!(matches, "ilp_address", String).expect("ilp_address is required");
                    let auth_token = value_t!(matches, "incoming_auth_token", String)
                        .expect("incoming_auth_token is required");
                    let ildcp_info = IldcpResponseBuilder {
                        client_address: &client_address.as_bytes(),
                        asset_code: "",
                        asset_scale: 0,
                    }
                    .build();
                    run_spsp_server_http(
                        ildcp_info,
                        ([127, 0, 0, 1], port).into(),
                        auth_token,
                        quiet,
                    );
                } else {
                    let btp_server = value_t!(matches, "btp_server", String)
                        .expect("BTP Server URL is required");
                    run_spsp_server_btp(&btp_server, ([127, 0, 0, 1], port).into(), quiet);
                }
            }
            ("pay", Some(matches)) => {
                let receiver = value_t!(matches, "receiver", String).expect("Receiver is required");
                let amount = value_t!(matches, "amount", u64).expect("Invalid amount");
                let quiet = matches.is_present("quiet");

                // Check for http_server first because btp_server has the default value of connecting to moneyd
                if let Ok(http_server) = value_t!(matches, "http_server", String) {
                    send_spsp_payment_http(&http_server, &receiver, amount, quiet)
                } else if let Ok(btp_server) = value_t!(matches, "btp_server", String) {
                    send_spsp_payment_btp(&btp_server, &receiver, amount, quiet);
                } else {
                    panic!("Must specify either btp_server or http_server");
                }
            }
            _ => app.print_help().unwrap(),
        },
        ("moneyd", Some(matches)) => match matches.subcommand() {
            ("local", Some(matches)) => {
                let btp_port = value_t!(matches, "port", u16).expect("btp_port is required");
                let ilp_address =
                    value_t!(matches, "ilp_address", String).expect("ilp_address is required");
                let asset_code =
                    value_t!(matches, "asset_code", String).expect("asset_code is required");
                let asset_scale =
                    value_t!(matches, "asset_scale", u8).expect("asset_scale is required");
                let ildcp_info = IldcpResponseBuilder {
                    client_address: ilp_address.as_str().as_bytes(),
                    asset_code: &asset_code,
                    asset_scale,
                }
                .build();
                run_moneyd_local(([127, 0, 0, 1], btp_port).into(), ildcp_info);
            }
            _ => app.print_help().unwrap(),
        },
        _ => app.print_help().unwrap(),
    }
}
