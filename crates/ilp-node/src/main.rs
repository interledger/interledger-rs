#![type_length_limit = "10000000"]
mod instrumentation;
pub mod node;

use cfg_if::cfg_if;

cfg_if! {
    if #[cfg(feature = "monitoring")] {
        use tracing_subscriber::{
            filter::EnvFilter,
            fmt::{time::ChronoUtc, Subscriber},
        };
        use node::LogWriter;
    }
}

#[cfg(feature = "redis")]
mod redis_store;

use clap::{App, Arg, ArgMatches};
use config::{Config, Source};
use config::{ConfigError, FileFormat, Value};
use libc::{c_int, isatty};
use node::InterledgerNode;
use std::{
    ffi::{OsStr, OsString},
    io::Read,
    vec::Vec,
};

#[tokio::main]
async fn main() {
    let version = format!(
        "{}-{}",
        env!("CARGO_PKG_VERSION"),
        env!("VERGEN_GIT_SHA_SHORT")
    );

    let app = cmdline_configuration(&version);
    let args = std::env::args_os().collect::<Vec<_>>();

    let additional_config = if !is_fd_tty(0) {
        // this might be read by load_configuration, depending on the presence of a config file
        let stdin = std::io::stdin();
        let lock = stdin.lock();
        Some(lock)
    } else {
        None
    };

    let node = match load_configuration(app, args, additional_config) {
        Ok(node) => node,
        Err(BadConfig::HelpOrVersion(e)) | Err(BadConfig::BadArguments(e)) => {
            e.exit();
        }
        Err(BadConfig::MergingStdinFailed(e)) => {
            output_config_error(e, None);
            std::process::exit(0);
        }
        Err(BadConfig::MergingConfigFileFailed(path, e)) => {
            output_config_error(e, Some(path.as_ref()));
            std::process::exit(0);
        }
        Err(BadConfig::ConversionFailed(e)) => {
            // this used to be an expect
            panic!("Could not parse provided configuration options into an Interledger Node config: {}", e);
        }
    };

    cfg_if! {
        if #[cfg(feature = "monitoring")] {
            let mut log_writer = LogWriter::default();

            let (nb_log_writer, _guard) = tracing_appender::non_blocking(log_writer.clone());

            let tracing_builder = Subscriber::builder()
                .with_timer(ChronoUtc::rfc3339())
                .with_env_filter(EnvFilter::from_default_env())
                .with_writer(nb_log_writer)
                .with_filter_reloading();

            log_writer.handle = Some(tracing_builder.reload_handle());

            let _ = tracing_builder.try_init();

            let log_writer = Some(log_writer);
        } else {
            let log_writer = None;
        }
    }

    node.serve(log_writer.clone()).await.unwrap();

    // Add a future which is always pending. This will ensure main does not exist
    // TODO: Is there a better way of doing this?
    futures::future::pending().await
}

fn cmdline_configuration(version: &str) -> clap::App<'static, '_> {
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
    // - Addresses to which ILP over HTTP or BTP servers are bound
    //     - `http_bind_address`
    // - Addresses to which other services are bound
    //     - `xxx_bind_address`
    App::new("ilp-node")
        .about("Run an Interledger.rs node (sender, connector, receiver bundle)")
        .version(version)
    .args(&[
        // Positional arguments
        Arg::with_name("config")
            .takes_value(true)
            .index(1)
            .help("Name of config file (in JSON or YAML format)"),
        // Non-positional arguments
        Arg::with_name("ilp_address")
            .long("ilp_address")
            .takes_value(true)
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
        Arg::with_name("database_url")
            .long("database_url")
            // temporary alias for backwards compatibility
            .alias("redis_url")
            .takes_value(true)
            .default_value("redis://127.0.0.1:6379")
            .help("Redis URI (for example, \"redis://127.0.0.1:6379\" or \"unix:/tmp/redis.sock\")"),
        Arg::with_name("database_prefix")
            .long("database_prefix")
            .takes_value(true)
            .default_value("")
            .help("Unique prefix that can be used to identify part of the db that this node will use. This can be used to enable multiple nodes to share the same database instance"),
        Arg::with_name("http_bind_address")
            .long("http_bind_address")
            .takes_value(true)
            .help("IP address and port to listen for HTTP connections. This is used for both the API and ILP over HTTP packets. ILP over HTTP is a means to transfer ILP packets instead of BTP connections"),
        Arg::with_name("settlement_api_bind_address")
            .long("settlement_api_bind_address")
            .takes_value(true)
            .help("IP address and port to listen for the Settlement Engine API"),
        Arg::with_name("default_spsp_account")
            .long("default_spsp_account")
            .takes_value(true)
            .help("When SPSP payments are sent to the root domain, the payment pointer is resolved to <domain>/.well-known/pay. This value determines which account those payments will be sent to."),
        Arg::with_name("route_broadcast_interval")
            .long("route_broadcast_interval")
            .takes_value(true)
            .help("Interval, defined in milliseconds, on which the node will broadcast routing information to other nodes using CCP. Defaults to 30000ms (30 seconds)."),
        Arg::with_name("exchange_rate.provider")
            .long("exchange_rate.provider")
            .takes_value(true)
            .help("Exchange rate API to poll for exchange rates. If this is not set, the node will not poll for rates and will instead use the rates set via the HTTP API. \
                Note that CryptoCompare can also be used when the node is configured via a config file or stdin, because an API key must be provided to use that service."),
        Arg::with_name("exchange_rate.poll_interval")
            .long("exchange_rate.poll_interval")
            .default_value("60000") // also change ExchangeRateConfig::default_poll_interval
            .help("Interval, defined in milliseconds, on which the node will poll the exchange_rate.provider (if specified) for exchange rates."),
        Arg::with_name("exchange_rate.spread")
            .long("exchange_rate.spread")
            .default_value("0") // also change ExchangeRateConfig::default_spread
            .help("Spread, as a fraction, to add on top of the exchange rate. \
                This amount is kept as the node operator's profit, or may cover \
                fluctuations in exchange rates.
                For example, take an incoming packet with an amount of 100. If the \
                exchange rate is 1:0.5 and the spread is 0.01, the amount on the \
                    outgoing packet would be 198 (instead of 200 without the spread)."),
        Arg::with_name("prometheus.bind_address")
            .long("prometheus.bind_address")
            .takes_value(true)
            .help("IP address and port to host the Prometheus endpoint on."),
        Arg::with_name("prometheus.histogram_window")
            .long("prometheus.histogram_window")
            .takes_value(true)
            .help("Amount of time, in milliseconds, that the node will collect data \
                points for the Prometheus histograms. Defaults to 300000ms (5 minutes)."),
        Arg::with_name("prometheus.histogram_granularity")
            .long("prometheus.histogram_granularity")
            .takes_value(true)
            .help("Granularity, in milliseconds, that the node will use to roll off \
                old data. For example, a value of 1000ms (1 second) would mean that the \
                node forgets the oldest 1 second of histogram data points every second. \
                Defaults to 10000ms (10 seconds)."),
        Arg::with_name("settle_every")
            .long("settle_every")
            .takes_value(true)
            .help("Settlement delay, in seconds; the peering accounts will be settled after \
                this many seconds after the first fulfill packet unless the balance had \
                exceeded the settlement threshold.\n\n\
                \
                Note: In a cluster configuration where multiple nodes share a \
                single database and database accounts, using this can result in \
                many settlements."),
        ])
}

#[derive(Debug)]
enum BadConfig {
    HelpOrVersion(clap::Error),
    BadArguments(clap::Error),
    MergingStdinFailed(config::ConfigError),
    MergingConfigFileFailed(String, config::ConfigError),
    ConversionFailed(config::ConfigError),
}

fn load_configuration<R: Read>(
    mut app: App<'_, '_>,
    args: Vec<OsString>,
    additional_config: Option<R>,
) -> Result<InterledgerNode, BadConfig> {
    let mut config = get_env_config("ilp");
    let (path, config_file) =
        precheck_arguments(app.clone(), &args).map_err(BadConfig::HelpOrVersion)?;

    if let Some(additional_config) = additional_config {
        merge_read_in(additional_config, &mut config).map_err(BadConfig::MergingStdinFailed)?;
    }

    if let Some(config_path) = config_file {
        merge_config_file(&config_path, &mut config)
            .map_err(|e| BadConfig::MergingConfigFileFailed(config_path, e))?;
    }

    set_app_env(&config, &mut app, &path, path.len());

    let matches = app
        .get_matches_from_safe(args.iter())
        .map_err(BadConfig::BadArguments)?;

    merge_args(&mut config, &matches);

    config.try_into().map_err(BadConfig::ConversionFailed)
}

fn output_config_error(error: ConfigError, config_path: Option<&str>) {
    let is_config_path_ilp_node = match config_path {
        Some(path) => path == "ilp-node",
        None => false,
    };

    match &error {
        ConfigError::PathParse(_) => println!("Error in parsing config: {:?}", error),

        // Note: configuring using a file called `ilp-node` is still allowed even though
        // `cargo run ilp-node` from the workspace root ends here; it only happens if there was an
        // error reading the file.
        _ if is_config_path_ilp_node => println!("Running ilp-node with `cargo run ilp-node` and \
                    `cargo run -p ilp-node` is deprecated. Please either execute the binary directly, or use \
                    `cargo run --bin ilp-node`"),
        _ => println!("Error: {:?}", error),
    }
}

/// Does early matching for command line arguments to get any given configuration file.
///
/// Returns (subcommand paths, config path)
fn precheck_arguments(
    mut app: App<'_, '_>,
    args: &[OsString],
) -> Result<(Vec<String>, Option<String>), clap::Error> {
    // not to cause `required fields error`.
    reset_required(&mut app);
    let matches = app.get_matches_from_safe_borrow(args.iter())?;
    let matches = &matches;
    let mut path = Vec::<String>::new();
    let subcommand = get_deepest_command(matches, &mut path);
    let mut config_path: Option<String> = None;
    if let Some(config_path_arg) = subcommand.value_of("config") {
        config_path = Some(config_path_arg.to_string());
    };
    Ok((path, config_path))
}

fn merge_config_file(config_path: &str, config: &mut Config) -> Result<(), ConfigError> {
    let file_config = config::File::with_name(config_path);
    let file_config = file_config.collect()?;
    // if the key is not defined in the given config already, set it to the config
    // because the original values override the ones from the config file
    for (k, v) in file_config {
        // Note: the error from `Config::get_str` is for "value was not found" or "couldn't be
        // turned into a string", not "the string representation is invalid for this key".
        if config.get_str(&k).is_err() {
            config.set(&k, v)?;
        }
    }

    Ok(())
}

fn merge_read_in<R: Read>(mut input: R, config: &mut Config) -> Result<(), ConfigError> {
    let mut buf = Vec::new();

    if let Ok(_read) = input.read_to_end(&mut buf) {
        if let Ok(buf_str) = String::from_utf8(buf) {
            let config_hash = FileFormat::Json
                .parse(None, &buf_str)
                .or_else(|_| FileFormat::Yaml.parse(None, &buf_str));
            if let Ok(config_hash) = config_hash {
                // if the key is not defined in the given config already, set it to the config
                // because the original values override the ones from the stdin
                for (k, v) in config_hash {
                    if config.get_str(&k).is_err() {
                        config.set(&k, v)?;
                    }
                }
            }
        }
    }
    Ok(())
}

fn merge_args(config: &mut Config, matches: &ArgMatches) {
    for (key, value) in &matches.args {
        if config.get_str(key).is_ok() {
            // Note: this means the key already pointed to a string convertable value, not that it
            // was valid for the property it configures.
            continue;
        }
        if value.vals.is_empty() {
            // flag
            // FIXME: this is never hit in the tests, unsure what it is for
            config.set(key, Value::new(None, true)).unwrap();
        } else {
            // merge the cmdline value to the config
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
        .merge(config::Environment::with_prefix(prefix).separator("__"))
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
fn set_app_env(env_config: &Config, app: &mut App<'_, '_>, path: &[String], depth: usize) {
    if depth == 1 {
        for item in &mut app.p.opts {
            if let Ok(value) = env_config.get_str(&item.b.name.to_lowercase()) {
                item.v.env = Some((OsStr::new(item.b.name), Some(OsString::from(value))));
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

fn reset_required(app: &mut App<'_, '_>) {
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
    // Because `isatty` is a `libc` function called using FFI, this is unsafe.
    // https://doc.rust-lang.org/book/ch19-01-unsafe-rust.html#using-extern-functions-to-call-external-code
    unsafe { isatty(file_descriptor) == 1 }
}

#[cfg(test)]
mod tests {
    use super::{cmdline_configuration, load_configuration, BadConfig, InterledgerNode};
    use std::ffi::OsString;
    use std::io::Write;

    #[test]
    fn loads_configuration_from_cmdline() {
        // as of writing this, there are two required parameters
        let args = [
            "ilp-node",
            "--admin_auth_token",
            "foobar",
            "--secret_seed",
            "8852500887504328225458511465394229327394647958135038836332350604",
        ]
        .iter()
        .map(OsString::from)
        .collect();
        let app = cmdline_configuration("anything");
        let additional = Option::<std::io::Empty>::None;

        let expected = serde_json::from_value::<InterledgerNode>(serde_json::json!({
            "admin_auth_token": "foobar",
            "secret_seed": "8852500887504328225458511465394229327394647958135038836332350604",
        }))
        .unwrap();

        let node = load_configuration(app, args, additional).unwrap();

        // this will start failing if the defaults for command line arguments do not match the
        // serde(default) values for the fields.
        assert_eq!(expected, node);
    }

    static ADDITIONAL_SECRETS: &[(&str, &[u8])] = &[
        ("json", b"{ \"secret_seed\": \"8852500887504328225458511465394229327394647958135038836332350604\" }"),
        ("yaml", b"secret_seed: \"8852500887504328225458511465394229327394647958135038836332350604\"\n"),
    ];

    static ADDITIONAL_AUTH_TOKEN: &[(&str, &[u8])] = &[
        ("json", b"{ \"admin_auth_token\": \"foobar\" }"),
        ("yaml", b"admin_auth_token: \"foobar\"\n"),
    ];

    #[test]
    fn loads_configuration_with_secret_over_stdin() {
        let args = ["ilp-node", "--admin_auth_token", "foobar"]
            .iter()
            .map(OsString::from)
            .collect::<Vec<_>>();
        let app = cmdline_configuration("anything");

        for (format, additional) in ADDITIONAL_SECRETS {
            let additional = Some(std::io::Cursor::new(additional));

            let expected = serde_json::from_value::<InterledgerNode>(serde_json::json!({
                "admin_auth_token": "foobar",
                "secret_seed": "8852500887504328225458511465394229327394647958135038836332350604",
            }))
            .unwrap();

            let node = load_configuration(app.clone(), args.clone(), additional)
                .unwrap_or_else(|e| panic!("with {}: {:?}", format, e));
            assert_eq!(expected, node, "secret_seed in {}", format);
        }
    }

    #[test]
    fn loads_configuration_from_cmdline_and_a_file() {
        for (format, additional) in ADDITIONAL_SECRETS {
            let mut named_temp = tempfile::Builder::new()
                .suffix(&format!(".{}", format))
                .tempfile()
                .unwrap();
            named_temp.write_all(additional).unwrap();
            named_temp.flush().unwrap();
            let name = named_temp.path().to_str().expect(
                "path wasn't UTF-8, but tempfile only uses alphanumeric random in the name",
            );

            let mut args = [
                "ilp-node",
                "--admin_auth_token",
                "foobar",
                "this_will_be_overridden_with_the_temp_config_file",
            ]
            .iter()
            .map(OsString::from)
            .collect::<Vec<_>>();

            {
                let last = args.last_mut().unwrap();
                last.clear();
                last.push(name);
            }

            let app = cmdline_configuration("anything");
            let additional = Option::<std::io::Empty>::None;
            let expected = serde_json::from_value::<InterledgerNode>(serde_json::json!({
                "admin_auth_token": "foobar",
                "secret_seed": "8852500887504328225458511465394229327394647958135038836332350604",
            }))
            .unwrap();

            let node = load_configuration(app, args, additional).unwrap();

            assert_eq!(expected, node);
        }
    }

    #[test]
    fn loads_configuration_from_cmdline_and_a_file_and_stdin() {
        let fixtures = ADDITIONAL_SECRETS.iter().zip(ADDITIONAL_AUTH_TOKEN.iter());

        for ((format, secret), (_, token)) in fixtures {
            let mut named_temp = tempfile::Builder::new()
                .suffix(&format!(".{}", format))
                .tempfile()
                .unwrap();
            named_temp.write_all(secret).unwrap();
            named_temp.flush().unwrap();
            let name = named_temp.path().to_str().expect(
                "path wasn't UTF-8, but tempfile only uses alphanumeric random in the name",
            );

            let mut args = [
                "ilp-node",
                "this_will_be_overridden_with_the_temp_config_file",
            ]
            .iter()
            .map(OsString::from)
            .collect::<Vec<_>>();

            {
                let last = args.last_mut().unwrap();
                last.clear();
                last.push(name);
            }

            let app = cmdline_configuration("anything");
            let additional = Some(std::io::Cursor::new(token));
            let expected = serde_json::from_value::<InterledgerNode>(serde_json::json!({
                "admin_auth_token": "foobar",
                "secret_seed": "8852500887504328225458511465394229327394647958135038836332350604",
            }))
            .unwrap();

            let node = load_configuration(app, args, additional).unwrap();

            assert_eq!(expected, node);
        }
    }

    /// Ensures the previous precedence is followed, which has been:
    ///
    /// 1. stdin input
    /// 2. config file
    /// 3. cmdline
    #[test]
    fn config_precedence() {
        // stdin json is the ...4 version, ...5 is on config file and ...6 is on cmdline
        let json = ADDITIONAL_SECRETS[0].1;

        let mut args = [
            "ilp-node",
            "--admin_auth_token",
            "foobar",
            "--secret_seed",
            "0000000000000000000000000000000000000000000000000000000000000006",
            "this_will_be_replaced_with_a_path_to_config_file",
        ]
        .iter()
        .map(OsString::from)
        .collect::<Vec<_>>();

        let mut named_temp = tempfile::Builder::new()
            .suffix(&".json")
            .tempfile()
            .unwrap();

        named_temp.write_all(&b"{\"secret_seed\":\"FFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFF5\"}\n"[..]).unwrap();
        named_temp.flush().unwrap();

        {
            let last = args.last_mut().unwrap();
            last.clear();
            last.push(named_temp.path());
        }

        let app = cmdline_configuration("anything");
        let additional = Some(std::io::Cursor::new(json));
        let expected = serde_json::from_value::<InterledgerNode>(serde_json::json!({
            "admin_auth_token": "foobar",
            "secret_seed": "8852500887504328225458511465394229327394647958135038836332350604",
        }))
        .unwrap();

        let node = load_configuration(app, args.clone(), additional).unwrap();

        assert_eq!(expected, node);

        // now without the stdin the config file should prevail

        let app = cmdline_configuration("anything");
        let additional = Option::<std::io::Empty>::None;
        let expected = serde_json::from_value::<InterledgerNode>(serde_json::json!({
            "admin_auth_token": "foobar",
            "secret_seed": "FFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFF5",
        }))
        .unwrap();

        let node = load_configuration(app, args, additional).unwrap();

        assert_eq!(expected, node);
    }

    #[test]
    fn invalid_preceding_values_error_out() {
        // command line is the only place where a valid secret_seed is provided, but the presence
        // rules force an error at the stdin or config file. the old implementation looked like it
        // allowed replacing invalid values but not in practice.
        let stdin = &b"{ \"secret_seed\": \"AAA\" }\n";

        let mut args = [
            "ilp-node",
            "--admin_auth_token",
            "foobar",
            "--secret_seed",
            "0000000000000000000000000000000000000000000000000000000000000006",
            "this_will_be_replaced_with_a_path_to_config_file",
        ]
        .iter()
        .map(OsString::from)
        .collect::<Vec<_>>();

        let mut named_temp = tempfile::Builder::new()
            .suffix(&".json")
            .tempfile()
            .unwrap();

        named_temp
            .write_all(&b"{\"secret_seed\":\"non_hex_this_is_invalid\"}\n"[..])
            .unwrap();
        named_temp.flush().unwrap();

        {
            let last = args.last_mut().unwrap();
            last.clear();
            last.push(named_temp.path());
        }

        let app = cmdline_configuration("anything");
        let additional = Some(std::io::Cursor::new(stdin));
        let bad = load_configuration(app, args.clone(), additional).unwrap_err();

        // the value from stdin is retained until the final deserialization (the file or cmdline
        // values are ignored as the stdin already contained a string convertable value).
        assert!(
            matches!(bad, BadConfig::ConversionFailed(_)),
            "unexpected: {:?}",
            bad
        );

        let app = cmdline_configuration("anything");
        let additional = Option::<std::io::Empty>::None;
        let bad = load_configuration(app, args, additional).unwrap_err();

        // same here, but the config file value is retained and cmdline value is ignored, for the
        // same reasons.
        assert!(
            matches!(bad, BadConfig::ConversionFailed(_)),
            "unexpected: {:?}",
            bad
        );
    }
}
