use clap::{crate_version, App, AppSettings, Arg, ArgMatches, SubCommand};
use config::{Config, Source};
use config::{FileFormat, Value};
use futures::Future;
use hex;
use interledger::node::{insert_account_redis, tokio_run, AccountDetails, InterledgerNode};
use interledger_packet::Address;
use interledger_service::Username;
use libc::{c_int, isatty};
use serde::Deserialize;
use std::ffi::{OsStr, OsString};
use std::io::Read;
use std::str::FromStr;
use std::vec::Vec;
use url::Url;

pub fn main() {
    env_logger::init();

    // The naming convention of arguments
    //
    // - URL vs URI
    //     - Basically it is recommended to use `URL` though both are technically correct.
    //       https://danielmiessler.com/study/url-uri/
    // - An address or a port
    //     - Use `xxx_bind_address` because it becomes more flexible than just binding it to
    //       `127.0.0.1` with a given port.
    // - ILP over HTTP or BTP server URLs which accept ILP packets
    //     - `http_server_url`
    //     - `btp_server_url`
    // - Addresses to which ILP over HTTP or BTP servers are bound
    //     - `http_bind_address`
    //     - `btp_bind_address`
    // - Addresses to which other services are bound
    //     - `xxx_bind_address`
    let mut app = App::new("interledger")
        .about("Blazing fast Interledger CLI written in Rust")
        .version(crate_version!())
        .setting(AppSettings::SubcommandsNegateReqs)
        // TODO remove this line once this issue is solved:
        // https://github.com/clap-rs/clap/issues/1536
        .after_help("")
        .subcommands(vec![
            SubCommand::with_name("node")
                .about("Run an Interledger node (sender, connector, receiver bundle)")
                .setting(AppSettings::SubcommandsNegateReqs)
                .args(&[
                    // Positional arguments
                    Arg::with_name("config")
                        .takes_value(true)
                        .index(1)
                        .help("Name of config file (in JSON, YAML, or TOML format)"),
                    // Non-positional arguments
                    Arg::with_name("ilp_address")
                        .long("ilp_address")
                        .takes_value(true)
                        .required(true)
                        .help("ILP Address of this account"),
                    Arg::with_name("secret_seed")
                        .long("secret_seed")
                        .takes_value(true)
                        .required(true)
                        .help("Root secret used to derive encryption keys. This MUST NOT be changed after once you started up the node. You can generate a random secret by running `openssl rand -hex 32`"),
                    Arg::with_name("admin_auth_token")
                        .long("admin_auth_token")
                        .takes_value(true)
                        .required(true)
                        .help("HTTP Authorization token for the node admin (sent as a Bearer token)"),
                    Arg::with_name("redis_url")
                        .long("redis_url")
                        .takes_value(true)
                        .default_value("redis://127.0.0.1:6379")
                        .help("Redis URI (for example, \"redis://127.0.0.1:6379\" or \"unix:/tmp/redis.sock\")"),
                    Arg::with_name("http_bind_address")
                        .long("http_bind_address")
                        .takes_value(true)
                        .help("IP address and port to listen for HTTP connections. This is used for both the API and ILP over HTTP packets. ILP over HTTP is a means to transfer ILP packets instead of BTP connections"),
                    Arg::with_name("settlement_api_bind_address")
                        .long("settlement_api_bind_address")
                        .takes_value(true)
                        .help("IP address and port to listen for the Settlement Engine API"),
                    Arg::with_name("btp_bind_address")
                        .long("btp_bind_address")
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
                    Arg::with_name("exchange_rate_provider")
                        .long("exchange_rate_provider")
                        .takes_value(true)
                        .possible_values(&["CoinCap"])
                        .help("Exchange rate API to poll for exchange rates. If this is not set, the node will not poll for rates and will instead use the rates set via the HTTP API. \
                            Note that CryptoCompare can also be used when the node is configured via a config file or stdin, because an API key must be provided to use that service."),
                    Arg::with_name("exchange_rate_poll_interval")
                        .long("exchange_rate_poll_interval")
                        .default_value("60000")
                        .help("Interval, defined in milliseconds, on which the node will poll the exchange_rate_provider (if specified) for exchange rates."),
                    Arg::with_name("exchange_rate_spread")
                        .long("exchange_rate_spread")
                        .default_value("0")
                        .help("Spread, as a fraction, to add on top of the exchange rate. \
                            This amount is kept as the node operator's profit, or may cover \
                            fluctuations in exchange rates. 
                            For example, take an incoming packet with an amount of 100. If the \
                            exchange rate is 1:2 and the spread is 0.01, the amount on the \
                            outgoing packet would be 198 (instead of 200 without the spread).")
                ])
                .subcommand(SubCommand::with_name("accounts")
                    .setting(AppSettings::SubcommandsNegateReqs)
                    .subcommand(SubCommand::with_name("add")
                        .args(&[
                            // Positional arguments
                            Arg::with_name("config")
                                .takes_value(true)
                                .index(1)
                                .help("Name of config file (in JSON, YAML, or TOML format)"),
                            // Non-positional arguments
                            Arg::with_name("redis_url")
                                .long("redis_url")
                                .default_value("redis://127.0.0.1:6379")
                                .help("Redis database to add the account to"),
                            Arg::with_name("secret_seed")
                                .long("secret_seed")
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
                            Arg::with_name("btp_server_url")
                                .long("btp_server_url")
                                .takes_value(true)
                                .help("URI of a BTP server or moneyd that this account should use to connect"),
                            Arg::with_name("http_server_url")
                                .long("http_server_url")
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
    if let Ok((path, config_file)) = precheck_arguments(app.clone()) {
        if !is_fd_tty(0) {
            merge_std_in(&mut config);
        }
        if let Some(ref config_path) = config_file {
            merge_config_file(config_path, &mut config);
        }
        set_app_env(&config, &mut app, &path, path.len());
    }
    let matches = app.clone().get_matches();
    match matches.subcommand() {
        ("node", Some(node_matches)) => match node_matches.subcommand() {
            ("accounts", Some(node_accounts_matches)) => match node_accounts_matches.subcommand() {
                ("add", Some(node_accounts_add_matches)) => {
                    merge_args(&mut config, &node_accounts_add_matches);
                    run_node_accounts_add(config.try_into::<NodeAccountsAddOpt>().unwrap());
                }
                _ => println!("{}", node_accounts_matches.usage()),
            },
            _ => {
                merge_args(&mut config, &node_matches);
                config.try_into::<InterledgerNode>().unwrap().run();
            }
        },
        ("", None) => app.print_help().unwrap(),
        _ => unreachable!(),
    }
}

// returns (subcommand paths, config path)
fn precheck_arguments(mut app: App) -> Result<(Vec<String>, Option<String>), ()> {
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
    let mut config_path: Option<String> = None;
    if let Some(config_path_arg) = subcommand.value_of("config") {
        config_path = Some(config_path_arg.to_string());
    };
    Ok((path, config_path))
}

fn merge_config_file(config_path: &str, config: &mut Config) {
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

fn merge_std_in(config: &mut Config) {
    let stdin = std::io::stdin();
    let mut stdin_lock = stdin.lock();
    let mut buf = Vec::new();
    if let Ok(_read) = stdin_lock.read_to_end(&mut buf) {
        if let Ok(buf_str) = String::from_utf8(buf) {
            let config_hash = FileFormat::Json
                .parse(None, &buf_str)
                .or_else(|_| FileFormat::Yaml.parse(None, &buf_str))
                .or_else(|_| FileFormat::Toml.parse(None, &buf_str))
                .ok();
            if let Some(config_hash) = config_hash {
                // if the key is not defined in the given config already, set it to the config
                // because the original values override the ones from the stdin
                for (k, v) in config_hash {
                    if config.get_str(&k).is_err() {
                        config.set(&k, v).unwrap();
                    }
                }
            }
        }
    }
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

// This sets the Config values which contains environment variables, config file settings, and STDIN
// settings, into each option's env value which is used when Parser parses the arguments. If this
// value is set, the Parser reads the value from it and doesn't warn even if the argument is not
// given from CLI.
// Usually `env` fn is used when creating `App` but this function automatically fills it so
// we don't need to call `env` fn manually.
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

// Check whether the file descriptor is pointed to TTY.
// For example, this function could be used to check whether the STDIN (fd: 0) is pointed to TTY.
// We use this function to check if we should read config from STDIN. If STDIN is NOT pointed to
// TTY, we try to read config from STDIN.
fn is_fd_tty(file_descriptor: c_int) -> bool {
    let result: c_int;
    // Because `isatty` is a `libc` function called using FFI, this is unsafe.
    // https://doc.rust-lang.org/book/ch19-01-unsafe-rust.html#using-extern-functions-to-call-external-code
    unsafe {
        result = isatty(file_descriptor);
    }
    result == 1
}

fn run_node_accounts_add(opt: NodeAccountsAddOpt) {
    let (http_endpoint, http_outgoing_token) = if let Some(url) = &opt.http_server_url {
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
    let redis_url = Url::parse(&opt.redis_url).expect("redis_url is not a valid URI");
    let server_secret: [u8; 32] = {
        let mut server_secret = [0; 32];
        let decoded = hex::decode(&opt.secret_seed).expect("secret_seed must be hex-encoded");
        assert_eq!(decoded.len(), 32, "secret_seed must be 32 bytes");
        server_secret.clone_from_slice(&decoded);
        server_secret
    };
    let account = AccountDetails {
        ilp_address: Some(Address::from_str(&opt.ilp_address).unwrap()),
        username: Username::from_str(&opt.username).unwrap(),
        asset_code: opt.asset_code.clone(),
        asset_scale: opt.asset_scale,
        btp_incoming_token: opt.btp_incoming_token.clone(),
        btp_uri: opt.btp_server_url.clone(),
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
        routing_relation: Some(opt.routing_relation.clone()),
        round_trip_time: Some(opt.round_trip_time),
        packets_per_minute_limit: opt.packets_per_minute_limit,
        amount_per_minute_limit: opt.amount_per_minute_limit,
        settlement_engine_url: None,
    };
    tokio_run(insert_account_redis(redis_url, &server_secret, account).and_then(move |_| Ok(())));
}

#[derive(Deserialize, Clone)]
struct NodeAccountsAddOpt {
    redis_url: String,
    secret_seed: String,
    ilp_address: String,
    username: String,
    asset_code: String,
    asset_scale: u8,
    btp_incoming_token: Option<String>,
    btp_server_url: Option<String>,
    http_server_url: Option<String>,
    http_incoming_token: Option<String>,
    settle_threshold: Option<i64>,
    settle_to: Option<i64>,
    routing_relation: String,
    min_balance: i64,
    round_trip_time: u32,
    packets_per_minute_limit: Option<u32>,
    amount_per_minute_limit: Option<u64>,
}
