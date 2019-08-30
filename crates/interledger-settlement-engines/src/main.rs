use clap::{crate_version, App, AppSettings, Arg, ArgMatches, SubCommand};
use config::{Config, ConfigError, Source, Value};
use serde::Deserialize;
use std::ffi::{OsStr, OsString};
use std::str::FromStr;
use tokio;
use url::Url;

use interledger_settlement_engines::engines::ethereum_ledger::{run_ethereum_engine, EthAddress};

pub fn main() {
    env_logger::init();

    let mut app = App::new("interledger-settlement-engines")
        .about("Interledger Settlement Engines CLI")
        .version(crate_version!())
        .setting(AppSettings::SubcommandsNegateReqs)
        .after_help("")
        .subcommands(vec![
            SubCommand::with_name("ethereum-ledger")
                .about("Ethereum settlement engine which performs ledger (layer 1) transactions")
                .setting(AppSettings::SubcommandsNegateReqs)
                .args(&[
                    Arg::with_name("config")
                        .takes_value(true)
                        .index(1)
                        .help("Name of config file (in JSON, TOML, YAML, or INI format)"),
                    Arg::with_name("http_address")
                        .long("http_address")
                        .takes_value(true)
                        .default_value("127.0.0.1:3000")
                        .help("Port to listen for settlement requests on"),
                    Arg::with_name("key")
                        .long("key")
                        .takes_value(true)
                        .required(true)
                        .help("private key for settlement account"),
                    Arg::with_name("ethereum_endpoint")
                        .long("ethereum_endpoint")
                        .takes_value(true)
                        .default_value("http://127.0.0.1:8545")
                        .help("Ethereum node endpoint. For example, the address of `ganache`"),
                    Arg::with_name("token_address")
                        .long("token_address")
                        .takes_value(true)
                        .default_value("")
                        .help("The address of the ERC20 token to be used for settlement (defaults to sending ETH if no token address is provided)"),
                    Arg::with_name("connector_url")
                        .long("connector_url")
                        .takes_value(true)
                        .help("Connector Settlement API endpoint")
                        .default_value("http://127.0.0.1:7771"),
                    Arg::with_name("redis_uri")
                        .long("redis_uri")
                        .takes_value(true)
                        .default_value("redis://127.0.0.1:6379")
                        .help("Redis database to add the account to"),
                    Arg::with_name("chain_id")
                        .long("chain_id")
                        .takes_value(true)
                        .default_value("1")
                        .help("The chain id so that the signer calculates the v value of the sig appropriately"),
                    Arg::with_name("confirmations")
                        .long("confirmations")
                        .takes_value(true)
                        .default_value("6")
                        .help("The number of confirmations the engine will wait for a transaction's inclusion before it notifies the node of its success"),
                    Arg::with_name("asset_scale")
                        .long("asset_scale")
                        .takes_value(true)
                        .default_value("18")
                        .help("The asset scale you want to use for your payments"),
                    Arg::with_name("poll_frequency")
                        .long("poll_frequency")
                        .takes_value(true)
                        .default_value("5000")
                        .help("The frequency in milliseconds at which the engine will check the blockchain about the confirmation status of a tx"),
                    Arg::with_name("watch_incoming")
                        .long("watch_incoming")
                        .default_value("true")
                        .help("Launch a blockchain watcher that listens for incoming transactions and notifies the connector upon sufficient confirmations"),
                ])
        ]);

    let mut config = get_env_config("ilp");
    if let Ok(path) = merge_config_file(app.clone(), &mut config) {
        set_app_env(&config, &mut app, &path, path.len());
    }

    let matches = app.clone().get_matches();
    let runner = Runner::new();
    match matches.subcommand() {
        ("ethereum-ledger", Some(ethereum_ledger_matches)) => {
            merge_args(&mut config, &ethereum_ledger_matches);
            runner.run(get_or_error(config.try_into::<EthereumLedgerOpt>()));
        }
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

// merge in other source of config options
// note that previously set values will NOT be overwritten
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

impl Runnable<EthereumLedgerOpt> for Runner {
    fn run(&self, opt: EthereumLedgerOpt) {
        let token_address: String = opt.token_address.clone();
        let token_address = if token_address.len() == 20 {
            Some(EthAddress::from_str(&token_address).unwrap())
        } else {
            None
        };
        let redis_uri = Url::parse(&opt.redis_uri).expect("redis_uri is not a valid URI");

        // TODO make key compatible with
        // https://github.com/tendermint/signatory to have HSM sigs

        tokio::run(run_ethereum_engine(
            redis_uri,
            opt.ethereum_endpoint.clone(),
            opt.http_address.parse().unwrap(),
            opt.key.clone(),
            opt.chain_id,
            opt.confirmations,
            opt.asset_scale,
            opt.poll_frequency,
            opt.connector_url.clone(),
            token_address,
            opt.watch_incoming,
        ));
    }
}

#[derive(Deserialize, Clone)]
struct EthereumLedgerOpt {
    key: String,
    http_address: String,
    ethereum_endpoint: String,
    token_address: String,
    connector_url: String,
    redis_uri: String,
    // Although the length of `chain_id` seems to be not limited on its specs,
    // u8 seems sufficient at this point.
    chain_id: u8,
    confirmations: u8,
    asset_scale: u8,
    poll_frequency: u64,
    watch_incoming: bool,
}
