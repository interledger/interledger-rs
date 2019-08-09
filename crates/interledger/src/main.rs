use base64;
use clap::value_t;
use clap::{App, Arg, ArgGroup, SubCommand};
use config;
use hex;
use interledger::{cli::*, node::*};
use interledger_ildcp::IldcpResponseBuilder;
use interledger_packet::Address;
use std::str::FromStr;
use tokio;
use url::Url;

#[allow(clippy::cognitive_complexity)]
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
                    .about("Run a local connector that exposes a BTP server with open signup")
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
                SubCommand::with_name("node")
                    .about("Run an Interledger node (sender, connector, receiver bundle)")
                    .arg(Arg::with_name("config")
                        .long("config")
                        .short("c")
                        .takes_value(true)
                        .help("Name of config file (in JSON, TOML, YAML, or INI format)"))
                    .subcommand(SubCommand::with_name("accounts")
                        .subcommand(SubCommand::with_name("add")
                        .args(&[
                            Arg::with_name("redis_uri")
                                .long("redis_uri")
                                .help("Redis database to add the account to")
                                .default_value("redis://127.0.0.1:6379"),
                            Arg::with_name("server_secret")
                                .long("server_secret")
                                .help("Cryptographic seed used to derive keys")
                                .takes_value(true)
                                .required(true),
                            Arg::with_name("ilp_address")
                                .long("ilp_address")
                                .help("ILP Address of this account")
                                .takes_value(true)
                                .required(true),
                            Arg::with_name("asset_code")
                                .long("asset_code")
                                .help("Asset that this account's balance is denominated in")
                                .takes_value(true)
                                .required(true),
                            Arg::with_name("asset_scale")
                                .long("asset_scale")
                                .help("Scale of the asset this account's balance is denominated in (a scale of 2 means that 100.50 will be represented as 10050)")
                                .takes_value(true)
                                .required(true),
                            Arg::with_name("btp_incoming_token")
                                .long("btp_incoming_token")
                                .help("BTP token this account will use to connect")
                                .takes_value(true),
                            Arg::with_name("btp_uri")
                                .long("btp_uri")
                                .help("URI of a BTP server or moneyd that this account should use to connect")
                                .takes_value(true),
                            Arg::with_name("http_url")
                                .help("URL of the ILP-Over-HTTP endpoint that should be used when sending outgoing requests to this account")
                                .long("http_url")
                                .takes_value(true),
                            Arg::with_name("http_incoming_token")
                                .long("http_incoming_token")
                                .help("Bearer token this account will use to authenticate HTTP requests sent to this server")
                                .takes_value(true),
                            Arg::with_name("settle_threshold")
                                .long("settle_threshold")
                                .help("Threshold, denominated in the account's asset and scale, at which an outgoing settlement should be sent")
                                .takes_value(true),
                            Arg::with_name("settle_to")
                                .long("settle_to")
                                .help("The amount that should be left after a settlement is triggered and sent (a negative value indicates that more should be sent than what is already owed)")
                                .takes_value(true),
                            Arg::with_name("send_routes")
                                .long("send_routes")
                                .help("Whether to broadcast routes to this account"),
                            Arg::with_name("receive_routes")
                                .long("receive_routes")
                                .help("Whether to accept route broadcasts from this account"),
                            Arg::with_name("routing_relation")
                                .long("routing_relation")
                                .help("Either 'Parent', 'Peer', or 'Child' to indicate our relationship to this account (used for routing)")
                                .default_value("Child"),
                            Arg::with_name("min_balance")
                                .long("min_balance")
                                .help("Minimum balance this account is allowed to have (can be negative)")
                                .default_value("0"),
                            Arg::with_name("round_trip_time")
                                .long("round_trip_time")
                                .help("The estimated amount of time (in milliseconds) we expect it to take to send a message to this account and receive the response")
                                .default_value("500"),
                            Arg::with_name("packets_per_minute_limit")
                                .long("packets_per_minute_limit")
                                .help("Number of outgoing Prepare packets per minute this account can send. Defaults to no limit")
                                .takes_value(true),
                            Arg::with_name("amount_per_minute_limit")
                                .long("amount_per_minute_limit")
                                .help("Total amount of value this account can send per minute. Defaults to no limit")
                                .takes_value(true),
                        ]))),
        ]);

    match app.clone().get_matches().subcommand() {
        ("spsp", Some(matches)) => match matches.subcommand() {
            ("server", Some(matches)) => {
                let port = value_t!(matches, "port", u16).expect("Invalid port");
                let quiet = matches.is_present("quiet");
                if matches.is_present("ilp_over_http") {
                    let client_address =
                        value_t!(matches, "ilp_address", String).expect("ilp_address is required");
                    let client_address = Address::from_str(&client_address).unwrap();
                    let auth_token = value_t!(matches, "incoming_auth_token", String)
                        .expect("incoming_auth_token is required");
                    let ildcp_info = IldcpResponseBuilder {
                        client_address: &client_address,
                        asset_code: "",
                        asset_scale: 0,
                    }
                    .build();
                    tokio::run(run_spsp_server_http(
                        ildcp_info,
                        ([127, 0, 0, 1], port).into(),
                        auth_token,
                        quiet,
                    ));
                } else {
                    let btp_server = value_t!(matches, "btp_server", String)
                        .expect("BTP Server URL is required");
                    tokio::run(run_spsp_server_btp(
                        &btp_server,
                        ([0, 0, 0, 0], port).into(),
                        quiet,
                    ));
                }
            }
            ("pay", Some(matches)) => {
                let receiver = value_t!(matches, "receiver", String).expect("Receiver is required");
                let amount = value_t!(matches, "amount", u64).expect("Invalid amount");
                let quiet = matches.is_present("quiet");

                // Check for http_server first because btp_server has the default value of connecting to moneyd
                if let Ok(http_server) = value_t!(matches, "http_server", String) {
                    tokio::run(send_spsp_payment_http(
                        &http_server,
                        &receiver,
                        amount,
                        quiet,
                    ));
                } else if let Ok(btp_server) = value_t!(matches, "btp_server", String) {
                    tokio::run(send_spsp_payment_btp(&btp_server, &receiver, amount, quiet));
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
                let ilp_address = Address::from_str(&ilp_address).unwrap();
                let asset_code =
                    value_t!(matches, "asset_code", String).expect("asset_code is required");
                let asset_scale =
                    value_t!(matches, "asset_scale", u8).expect("asset_scale is required");

                let ildcp_info = IldcpResponseBuilder {
                    client_address: &ilp_address,
                    asset_code: &asset_code,
                    asset_scale,
                }
                .build();
                tokio::run(run_moneyd_local(
                    ([127, 0, 0, 1], btp_port).into(),
                    ildcp_info,
                ));
            }
            _ => app.print_help().unwrap(),
        },
        ("node", Some(matches)) => match matches.subcommand() {
            ("accounts", Some(matches)) => match matches.subcommand() {
                ("add", Some(matches)) => {
                    let (http_endpoint, http_outgoing_token) =
                        if let Some(url) = matches.value_of("http_url") {
                            let url = Url::parse(url).expect("Invalid URL");
                            let auth = if !url.username().is_empty() {
                                Some(format!(
                                    "Basic {}",
                                    base64::encode(&format!(
                                        "{}:{}",
                                        url.username(),
                                        url.password().unwrap_or("")
                                    ))
                                ))
                            } else if let Some(password) = url.password() {
                                Some(format!("Bearer {}", password))
                            } else {
                                None
                            };
                            (Some(url.to_string()), auth)
                        } else {
                            (None, None)
                        };
                    let redis_uri =
                        value_t!(matches, "redis_uri", String).expect("redis_uri is required");
                    let redis_uri = Url::parse(&redis_uri).expect("redis_uri is not a valid URI");
                    let server_secret: [u8; 32] = {
                        let encoded: String = value_t!(matches, "server_secret", String).unwrap();
                        let mut server_secret = [0; 32];
                        let decoded =
                            hex::decode(encoded).expect("server_secret must be hex-encoded");
                        assert_eq!(decoded.len(), 32, "server_secret must be 32 bytes");
                        server_secret.clone_from_slice(&decoded);
                        server_secret
                    };
                    let account = AccountDetails {
                        ilp_address: Address::from_str(
                            &value_t!(matches, "ilp_address", String).unwrap(),
                        )
                        .unwrap(),
                        asset_code: value_t!(matches, "asset_code", String).unwrap(),
                        asset_scale: value_t!(matches, "asset_scale", u8).unwrap(),
                        btp_incoming_token: matches
                            .value_of("btp_incoming_token")
                            .map(|s| s.to_string()),
                        btp_uri: matches.value_of("btp_uri").map(|s| s.to_string()),
                        http_incoming_token: matches
                            .value_of("http_incoming_token")
                            .map(|s| format!("Bearer {}", s)),
                        http_outgoing_token,
                        http_endpoint,
                        max_packet_amount: u64::max_value(),
                        min_balance: value_t!(matches, "min_balance", i64).ok(),
                        settle_threshold: value_t!(matches, "settle_threshold", i64).ok(),
                        settle_to: value_t!(matches, "settle_to", i64).ok(),
                        send_routes: matches.is_present("send_routes"),
                        receive_routes: matches.is_present("receive_routes"),
                        routing_relation: value_t!(matches, "routing_relation", String).ok(),
                        round_trip_time: value_t!(matches, "round_trip_time", u64).ok(),
                        packets_per_minute_limit: value_t!(
                            matches,
                            "packets_per_minute_limit",
                            u32
                        )
                        .ok(),
                        amount_per_minute_limit: value_t!(matches, "amount_per_minute_limit", u64)
                            .ok(),
                        settlement_engine_url: None,
                    };
                    tokio::run(insert_account_redis(redis_uri, &server_secret, account));
                }
                _ => app.print_help().unwrap(),
            },
            _ => {
                let mut node_config = config::Config::new();
                if let Some(config_path) = matches.value_of("config") {
                    node_config
                        .merge(config::File::with_name(config_path))
                        .unwrap();
                }
                node_config
                    .merge(config::Environment::with_prefix("ILP"))
                    .unwrap();

                let node: InterledgerNode = node_config
                    .try_into()
                    .expect("Must provide config file name or config environment variables");
                node.run();
            }
        },
        _ => app.print_help().unwrap(),
    }
}
