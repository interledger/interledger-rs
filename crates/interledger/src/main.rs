use base64;
use clap::{crate_version, App, AppSettings, Arg, ArgGroup, ArgMatches, SubCommand};
use config::Value;
use config::{Config, ConfigError, Source};
use futures::future::Future;
use hex;
use interledger::{cli::*, node::*};
use interledger_ildcp::IldcpResponseBuilder;
use interledger_packet::Address;
use interledger_service::Username;
use serde::Deserialize;
use std::ffi::{OsStr, OsString};
use std::str::FromStr;
use tokio;
use url::Url;

pub fn main() {
    env_logger::init();

    let moneyd_uri = format!(
        "btp+ws://{}:{}@localhost:7768",
        random_token(),
        random_token()
    );

    let mut app = App::new("interledger")
        .about("Blazing fast Interledger CLI written in Rust")
        .version(crate_version!())
        .setting(AppSettings::SubcommandsNegateReqs)
        .after_help("")
        .subcommands(vec![
            SubCommand::with_name("spsp")
                .about("Client and Server for the Simple Payment Setup Protocol (SPSP)")
                .setting(AppSettings::SubcommandsNegateReqs)
                .subcommands(vec![
                    SubCommand::with_name("server")
                        .about("Run an SPSP Server that automatically accepts incoming money")
                        .args(&[
                            Arg::with_name("config")
                                .takes_value(true)
                                .index(1)
                                .help("Name of config file (in JSON, TOML, YAML, or INI format)"),
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
                                .required(true)
                                .help("The server's ILP address (Required for ilp_over_http)"),
                            Arg::with_name("incoming_auth_token")
                                .long("incoming_auth_token")
                                .takes_value(true)
                                .required(true)
                                .help("Token that must be used to authenticate incoming requests (Required for ilp_over_http)"),
                            Arg::with_name("quiet")
                                .long("quiet")
                                .help("Suppress log output"),
                        ])
                        .group(ArgGroup::with_name("http_options").requires_all(&["ilp_over_http", "ilp_address", "incoming_auth_token"])),
                    SubCommand::with_name("pay")
                        .about("Send an SPSP payment")
                        .args(&[
                            Arg::with_name("config")
                                .takes_value(true)
                                .index(1)
                                .help("Name of config file (in JSON, TOML, YAML, or INI format)"),
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
                .setting(AppSettings::SubcommandsNegateReqs)
                .subcommand(SubCommand::with_name("local")
                    .about("Run locally without connecting to a remote connector")
                    .args(&[
                        Arg::with_name("config")
                            .takes_value(true)
                            .index(1)
                            .help("Name of config file (in JSON, TOML, YAML, or INI format)"),
                        Arg::with_name("port")
                            .long("port")
                            .short("p")
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
                            .help("Scale of the asset this account's balance is denominated in (a scale of 2 means that 100.50 will be represented as 10050) Refer to https://bit.ly/2ZlOy9n")
                            .default_value("9"),
                    ])
                ),
            SubCommand::with_name("node")
                .about("Run an Interledger node (sender, connector, receiver bundle)")
                .setting(AppSettings::SubcommandsNegateReqs)
                .args(&[
                    Arg::with_name("config")
                        .takes_value(true)
                        .index(1)
                        .help("Name of config file (in JSON, TOML, YAML, or INI format)"),
                    Arg::with_name("ilp_address")
                        .long("ilp_address")
                        .takes_value(true)
                        .required(true)
                        .help("ILP Address of this account"),
                    Arg::with_name("secret_seed")
                        .long("secret_seed")
                        .takes_value(true)
                        .required(true)
                        .help("Root secret used to derive encryption keys. This MUST NOT be changed after once you started up the node. You could obtain randomly generated one using `openssl rand -hex 32`"),
                    Arg::with_name("admin_auth_token")
                        .long("admin_auth_token")
                        .takes_value(true)
                        .required(true)
                        .help("HTTP Authorization token for the node admin (sent as a Bearer token). Refer to: https://bit.ly/2Lk4xgF https://bit.ly/2XaUiBb"),
                    Arg::with_name("redis_connection")
                        .long("redis_connection")
                        .takes_value(true)
                        .default_value("redis://127.0.0.1:6379")
                        .help("Redis URI (for example, \"redis://127.0.0.1:6379\" or \"unix:/tmp/redis.sock\")"),
                    Arg::with_name("http_address")
                        .long("http_address")
                        .takes_value(true)
                        .help("IP address and port to listen for HTTP connections. This is used for both the API and ILP over HTTP packets. ILP over HTTP is a means to transfer ILP packets instead of BTP connections"),
                    Arg::with_name("settlement_address")
                        .long("settlement_address")
                        .takes_value(true)
                        .help("IP address and port to listen for the Settlement Engine API"),
                    Arg::with_name("btp_address")
                        .long("btp_address")
                        .takes_value(true)
                        .help("IP address and port to listen for BTP connections"),
                    Arg::with_name("default_spsp_account")
                        .long("default_spsp_account")
                        .takes_value(true)
                        .help("When SPSP payments are sent to the root domain, the payment pointer is resolved to <domain>/.well-known/pay. This value determines which account those payments will be sent to."),
                    Arg::with_name("route_broadcast_interval")
                        .long("route_broadcast_interval")
                        .takes_value(true)
                        .help("Interval, defined in milliseconds, on which the node will broadcast routing information to other nodes using CCP. Defaults to 30000ms (30 seconds)."),
                ])
                .subcommand(SubCommand::with_name("accounts")
                    .setting(AppSettings::SubcommandsNegateReqs)
                    .subcommand(SubCommand::with_name("add")
                        .args(&[
                            Arg::with_name("config")
                                .takes_value(true)
                                .index(1)
                                .help("Name of config file (in JSON, TOML, YAML, or INI format)"),
                            Arg::with_name("redis_uri")
                                .long("redis_uri")
                                .default_value("redis://127.0.0.1:6379")
                                .help("Redis database to add the account to"),
                            Arg::with_name("server_secret")
                                .long("server_secret")
                                .takes_value(true)
                                .required(true)
                                .help("Cryptographic seed used to derive keys"),
                            Arg::with_name("ilp_address")
                                .long("ilp_address")
                                .takes_value(true)
                                .required(true)
                                .help("ILP Address of this account"),
                            Arg::with_name("username")
                                .long("username")
                                .takes_value(true)
                                .required(true)
                                .help("Username of this account"),
                            Arg::with_name("asset_code")
                                .long("asset_code")
                                .takes_value(true)
                                .required(true)
                                .help("Asset that this account's balance is denominated in"),
                            Arg::with_name("asset_scale")
                                .long("asset_scale")
                                .takes_value(true)
                                .required(true)
                                .help("Scale of the asset this account's balance is denominated in (a scale of 2 means that 100.50 will be represented as 10050) Refer to https://bit.ly/2ZlOy9n"),
                            Arg::with_name("btp_incoming_token")
                                .long("btp_incoming_token")
                                .takes_value(true)
                                .help("BTP token this account will use to connect"),
                            Arg::with_name("btp_uri")
                                .long("btp_uri")
                                .takes_value(true)
                                .help("URI of a BTP server or moneyd that this account should use to connect"),
                            Arg::with_name("http_url")
                                .long("http_url")
                                .takes_value(true)
                                .help("URL of the ILP-Over-HTTP endpoint that should be used when sending outgoing requests to this account"),
                            Arg::with_name("http_incoming_token")
                                .long("http_incoming_token")
                                .takes_value(true)
                                .help("Bearer token this account will use to authenticate HTTP requests sent to this server"),
                            Arg::with_name("settle_threshold")
                                .long("settle_threshold")
                                .takes_value(true)
                                .help("Threshold, denominated in the account's asset and scale, at which an outgoing settlement should be sent"),
                            Arg::with_name("settle_to")
                                .long("settle_to")
                                .takes_value(true)
                                .help("The amount that should be left after a settlement is triggered and sent (a negative value indicates that more should be sent than what is already owed)"),
                            Arg::with_name("send_routes")
                                .long("send_routes")
                                .help("Whether to broadcast routes to this account"),
                            Arg::with_name("receive_routes")
                                .long("receive_routes")
                                .help("Whether to accept route broadcasts from this account"),
                            Arg::with_name("routing_relation")
                                .long("routing_relation")
                                .default_value("Child")
                                .help("Either 'Parent', 'Peer', or 'Child' to indicate our relationship to this account (used for routing)"),
                            Arg::with_name("min_balance")
                                .long("min_balance")
                                .default_value("0")
                                .help("Minimum balance this account is allowed to have (can be negative)"),
                            Arg::with_name("round_trip_time")
                                .long("round_trip_time")
                                .default_value("500")
                                .help("The estimated amount of time (in milliseconds) we expect it to take to send a message to this account and receive the response"),
                            Arg::with_name("packets_per_minute_limit")
                                .long("packets_per_minute_limit")
                                .takes_value(true)
                                .help("Number of outgoing Prepare packets per minute this account can send. Defaults to no limit"),
                            Arg::with_name("amount_per_minute_limit")
                                .long("amount_per_minute_limit")
                                .takes_value(true)
                                .help("Total amount of value this account can send per minute. Defaults to no limit"),
                        ])
                    )
                )
        ]);

    let mut config = get_env_config("ilp");
    if let Ok(path) = merge_config_file(app.clone(), &mut config) {
        set_app_env(&config, &mut app, &path, path.len());
    }

    let matches = app.clone().get_matches();
    let runner = Runner::new();
    match matches.subcommand() {
        ("spsp", Some(spsp_matches)) => match spsp_matches.subcommand() {
            ("server", Some(spsp_server_matches)) => {
                merge_args(&mut config, &spsp_server_matches);
                runner.run(get_or_error(config.try_into::<SpspServerOpt>()));
            }
            ("pay", Some(spsp_pay_matches)) => {
                merge_args(&mut config, &spsp_pay_matches);
                runner.run(get_or_error(config.try_into::<SpspPayOpt>()));
            }
            _ => println!("{}", spsp_matches.usage()),
        },
        ("moneyd", Some(moneyd_matches)) => match moneyd_matches.subcommand() {
            ("local", Some(moneyd_local_matches)) => {
                merge_args(&mut config, &moneyd_local_matches);
                runner.run(get_or_error(config.try_into::<MoneydLocalOpt>()));
            }
            _ => println!("{}", moneyd_matches.usage()),
        },
        ("node", Some(node_matches)) => match node_matches.subcommand() {
            ("accounts", Some(node_accounts_matches)) => match node_accounts_matches.subcommand() {
                ("add", Some(node_accounts_add_matches)) => {
                    merge_args(&mut config, &node_accounts_add_matches);
                    runner.run(get_or_error(config.try_into::<NodeAccountsAddOpt>()));
                }
                _ => println!("{}", node_accounts_matches.usage()),
            },
            _ => {
                merge_args(&mut config, &node_matches);
                get_or_error(config.try_into::<InterledgerNode>()).run();
            }
        },
        ("", None) => app.print_help().unwrap(),
        _ => unreachable!(),
    }
}

fn merge_config_file(mut app: App, config: &mut Config) -> Result<Vec<String>, ()> {
    // not to cause `required fields error`.
    reset_required(&mut app);
    let matches = app.get_matches_safe();
    if matches.is_err() {
        // if app could not get any appropriate match, just return not to show help etc.
        return Err(());
    }
    let matches = &matches.unwrap();
    let mut path = Vec::<String>::new();
    let subcommand = get_deepest_command(matches, &mut path);
    if let Some(config_path) = subcommand.value_of("config") {
        let file_config = config::File::with_name(config_path);
        let file_config = file_config.collect().unwrap();

        // if the key is not defined in the given config already, set it to the config
        // because the original values override the ones from the config file
        for (k, v) in file_config {
            if config.get_str(&k).is_err() {
                config.set(&k, v).unwrap();
            }
        }
    }
    Ok(path)
}

fn merge_args(config: &mut Config, matches: &ArgMatches) {
    for (key, value) in &matches.args {
        if config.get_str(key).is_ok() {
            continue;
        }
        if value.vals.is_empty() {
            // flag
            config.set(key, Value::new(None, true)).unwrap();
        } else {
            // value
            config
                .set(key, Value::new(None, value.vals[0].to_str().unwrap()))
                .unwrap();
        }
    }
}

// retrieve Config from a certain prefix
// if the prefix is `ilp`, `address` is resolved to `ilp_address`
fn get_env_config(prefix: &str) -> Config {
    let mut config = Config::new();
    config
        .merge(config::Environment::with_prefix(prefix))
        .unwrap();

    if prefix.to_lowercase() == "ilp" {
        if let Ok(value) = config.get_str("address") {
            config.set("ilp_address", value).unwrap();
        }
    }

    config
}

// sets env value into each optional value
// only applied to the specified last command
fn set_app_env(env_config: &Config, app: &mut App, path: &[String], depth: usize) {
    if depth == 1 {
        for item in &mut app.p.opts {
            if let Ok(value) = env_config.get_str(&item.b.name.to_lowercase()) {
                item.v.env = Some((&OsStr::new(item.b.name), Some(OsString::from(value))));
            }
        }
        return;
    }
    for subcommand in &mut app.p.subcommands {
        if subcommand.get_name() == path[path.len() - depth] {
            set_app_env(env_config, subcommand, path, depth - 1);
        }
    }
}

fn get_deepest_command<'a>(matches: &'a ArgMatches, path: &mut Vec<String>) -> &'a ArgMatches<'a> {
    let (name, subcommand_matches) = matches.subcommand();
    path.push(name.to_string());
    if let Some(matches) = subcommand_matches {
        return get_deepest_command(matches, path);
    }
    matches
}

fn reset_required(app: &mut App) {
    app.p.required.clear();
    for subcommand in &mut app.p.subcommands {
        reset_required(subcommand);
    }
}

fn get_or_error<T>(item: Result<T, ConfigError>) -> T {
    match item {
        Ok(item) => item,
        Err(error) => {
            match error {
                ConfigError::Message(message) => eprintln!("Configuration error: {:?}", message),
                _ => eprintln!("{:?}", error),
            };
            std::process::exit(1);
        }
    }
}

struct Runner {}

impl Runner {
    fn new() -> Runner {
        Runner {}
    }
}

trait Runnable<T> {
    fn run(&self, opt: T);
}

impl Runnable<SpspServerOpt> for Runner {
    fn run(&self, opt: SpspServerOpt) {
        if opt.ilp_over_http {
            let ildcp_info = IldcpResponseBuilder {
                client_address: &Address::from_str(&opt.ilp_address).unwrap(),
                asset_code: "",
                asset_scale: 0,
            }
            .build();
            tokio::run(run_spsp_server_http(
                ildcp_info,
                ([127, 0, 0, 1], opt.port).into(),
                opt.incoming_auth_token.clone(),
                opt.quiet,
            ));
        } else {
            tokio::run(run_spsp_server_btp(
                &opt.btp_server,
                ([0, 0, 0, 0], opt.port).into(),
                opt.quiet,
            ));
        }
    }
}

impl Runnable<SpspPayOpt> for Runner {
    fn run(&self, opt: SpspPayOpt) {
        // Check for http_server first because btp_server has the default value of connecting to moneyd
        if let Some(http_server) = &opt.http_server {
            tokio::run(send_spsp_payment_http(
                http_server,
                &opt.receiver,
                opt.amount,
                opt.quiet,
            ));
        } else if let Some(btp_server) = &opt.btp_server {
            tokio::run(send_spsp_payment_btp(
                btp_server,
                &opt.receiver,
                opt.amount,
                opt.quiet,
            ));
        } else {
            panic!("Must specify either btp_server or http_server");
        }
    }
}

impl Runnable<MoneydLocalOpt> for Runner {
    fn run(&self, opt: MoneydLocalOpt) {
        let ildcp_info = IldcpResponseBuilder {
            client_address: &Address::from_str(&opt.ilp_address).unwrap(),
            asset_code: &opt.asset_code,
            asset_scale: opt.asset_scale,
        }
        .build();
        tokio::run(run_moneyd_local(
            ([127, 0, 0, 1], opt.port).into(),
            ildcp_info,
        ));
    }
}

impl Runnable<NodeAccountsAddOpt> for Runner {
    fn run(&self, opt: NodeAccountsAddOpt) {
        let (http_endpoint, http_outgoing_token) = if let Some(url) = &opt.http_url {
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
        let redis_uri = Url::parse(&opt.redis_uri).expect("redis_uri is not a valid URI");
        let server_secret: [u8; 32] = {
            let mut server_secret = [0; 32];
            let decoded =
                hex::decode(&opt.server_secret).expect("server_secret must be hex-encoded");
            assert_eq!(decoded.len(), 32, "server_secret must be 32 bytes");
            server_secret.clone_from_slice(&decoded);
            server_secret
        };
        let account = AccountDetails {
            ilp_address: Address::from_str(&opt.ilp_address).unwrap(),
            username: Username::from_str(&opt.username).unwrap(),
            asset_code: opt.asset_code.clone(),
            asset_scale: opt.asset_scale,
            btp_incoming_token: opt.btp_incoming_token.clone(),
            btp_uri: opt.btp_uri.clone(),
            http_incoming_token: opt
                .http_incoming_token
                .clone()
                .map(|s| format!("Bearer {}", s)),
            http_outgoing_token,
            http_endpoint,
            max_packet_amount: u64::max_value(),
            min_balance: Some(opt.min_balance),
            settle_threshold: opt.settle_threshold,
            settle_to: opt.settle_to,
            send_routes: opt.send_routes,
            receive_routes: opt.receive_routes,
            routing_relation: Some(opt.routing_relation.clone()),
            round_trip_time: Some(opt.round_trip_time),
            packets_per_minute_limit: opt.packets_per_minute_limit,
            amount_per_minute_limit: opt.amount_per_minute_limit,
            settlement_engine_url: None,
        };
        tokio::run(
            insert_account_redis(redis_uri, &server_secret, account).and_then(move |_| Ok(())),
        );
    }
}

#[derive(Deserialize, Clone)]
struct SpspServerOpt {
    port: u16,
    btp_server: String,
    #[serde(default = "default_as_false")]
    ilp_over_http: bool,
    ilp_address: String,
    incoming_auth_token: String,
    #[serde(default = "default_as_false")]
    quiet: bool,
}

#[derive(Deserialize, Clone)]
struct SpspPayOpt {
    btp_server: Option<String>,
    http_server: Option<String>,
    receiver: String,
    amount: u64,
    #[serde(default = "default_as_false")]
    quiet: bool,
}

#[derive(Deserialize, Clone)]
struct MoneydLocalOpt {
    port: u16,
    ilp_address: String,
    asset_code: String,
    asset_scale: u8,
}

#[derive(Deserialize, Clone)]
struct NodeAccountsAddOpt {
    redis_uri: String,
    server_secret: String,
    ilp_address: String,
    username: String,
    asset_code: String,
    asset_scale: u8,
    btp_incoming_token: Option<String>,
    btp_uri: Option<String>,
    http_url: Option<String>,
    http_incoming_token: Option<String>,
    settle_threshold: Option<i64>,
    settle_to: Option<i64>,
    #[serde(default = "default_as_false")]
    send_routes: bool,
    #[serde(default = "default_as_false")]
    receive_routes: bool,
    routing_relation: String,
    min_balance: i64,
    round_trip_time: u32,
    packets_per_minute_limit: Option<u32>,
    amount_per_minute_limit: Option<u64>,
}

fn default_as_false() -> bool {
    false
}
