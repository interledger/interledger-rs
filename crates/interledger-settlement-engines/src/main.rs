use clap::{crate_version, App, AppSettings, ArgMatches};
use config::{Config, FileFormat, Source};
use futures::Future;
use libc::{c_int, isatty};
use std::ffi::{OsStr, OsString};
use std::io::Read;
use std::vec::Vec;

#[cfg(feature = "ethereum")]
use clap::{Arg, SubCommand};

#[cfg(feature = "ethereum")]
use config::Value;

#[cfg(feature = "ethereum")]
use interledger_settlement_engines::engines::ethereum_ledger::{
    run_ethereum_engine, EthereumLedgerOpt,
};

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
    //     - `ilp_over_http_url`
    //     - `ilp_over_btp_url`
    // - Addresses to which ILP over HTTP servers are bound
    //     - `http_bind_address`
    // - Addresses to which other services are bound
    //     - `xxx_bind_address`
    let mut app = App::new("interledger-settlement-engines")
        .about("Interledger Settlement Engines CLI")
        .version(crate_version!())
        .setting(AppSettings::SubcommandsNegateReqs)
        // TODO remove this line once this issue is solved:
        // https://github.com/clap-rs/clap/issues/1536
        .after_help("")
        .subcommands(vec![
            #[cfg(feature = "ethereum")]
            SubCommand::with_name("ethereum-ledger")
                .about("Ethereum settlement engine which performs ledger (layer 1) transactions")
                .setting(AppSettings::SubcommandsNegateReqs)
                .args(&[
                    // Positional arguments
                    Arg::with_name("config")
                        .takes_value(true)
                        .index(1)
                        .help("Name of config file (in JSON, YAML, or TOML format)"),
                    // Non-positional arguments
                    Arg::with_name("settlement_api_bind_address")
                        .long("settlement_api_bind_address")
                        .takes_value(true)
                        .default_value("127.0.0.1:3000")
                        .help("Port to listen for settlement requests on"),
                    Arg::with_name("private_key")
                        .long("private_key")
                        .takes_value(true)
                        .required(true)
                        .help("Ethereum private key for settlement account"),
                    Arg::with_name("ethereum_url")
                        .long("ethereum_url")
                        .takes_value(true)
                        .default_value("http://127.0.0.1:8545")
                        .help("Ethereum node endpoint URL. For example, the address of `ganache`"),
                    Arg::with_name("token_address")
                        .long("token_address")
                        .takes_value(true)
                        .help("The address of the ERC20 token to be used for settlement (defaults to sending ETH if no token address is provided)"),
                    Arg::with_name("connector_url")
                        .long("connector_url")
                        .takes_value(true)
                        .help("Connector Settlement API endpoint")
                        .default_value("http://127.0.0.1:7771"),
                    Arg::with_name("redis_url")
                        .long("redis_url")
                        .takes_value(true)
                        .default_value("redis://127.0.0.1:6379")
                        .help("Redis database to add the account to"),
                    Arg::with_name("chain_id")
                        .long("chain_id")
                        .takes_value(true)
                        .default_value("1")
                        .help("The chain id so that the signer calculates the v value of the sig appropriately. Defaults to 1 which means the mainnet. There are some other options such as: 3(Ropsten: PoW testnet), 4(Rinkeby: PoA testnet), etc."),
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
        #[cfg(feature = "ethereum")]
        ("ethereum-ledger", Some(ethereum_ledger_matches)) => {
            merge_args(&mut config, &ethereum_ledger_matches);
            tokio_run(run_ethereum_engine(
                config.try_into::<EthereumLedgerOpt>().unwrap(),
            ));
        }
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

#[cfg(feature = "ethereum")]
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

#[doc(hidden)]
pub fn tokio_run(fut: impl Future<Item = (), Error = ()> + Send + 'static) {
    let mut runtime = tokio::runtime::Builder::new()
        // Don't swallow panics
        .panic_handler(|err| std::panic::resume_unwind(err))
        .name_prefix("interledger-rs-worker-")
        .build()
        .expect("failed to start new runtime");

    runtime.spawn(fut);
    runtime.shutdown_on_idle().wait().unwrap();
}
