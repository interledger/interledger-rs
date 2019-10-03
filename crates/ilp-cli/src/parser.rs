use clap::{crate_version, App, AppSettings, Arg, SubCommand};

pub fn build<'a, 'b>() -> App<'a, 'b> {
    ilp_cli().subcommands(vec![
        accounts().subcommands(vec![
            accounts_balance(),
            accounts_create(),
            accounts_delete(),
            accounts_incoming_payments(),
            accounts_info(),
            accounts_list(),
            accounts_update(),
            accounts_update_settings(),
        ]),
        pay(),
        rates().subcommands(vec![rates_list(), rates_set_all()]),
        routes().subcommands(vec![routes_list(), routes_set(), routes_set_all()]),
        settlement_engines().subcommands(vec![settlement_engines_set_all()]),
        status(),
        testnet().subcommands(vec![testnet_setup()]),
    ])
}

struct AuthorizedSubCommand;

impl AuthorizedSubCommand {
    fn with_name(name: &str) -> App {
        SubCommand::with_name(name).arg(
            Arg::with_name("authorization_key")
                .long("auth")
                .env("ILP_CLI_API_AUTH")
                .required(true)
                .help("An HTTP bearer authorization token permitting access to this operation"),
        )
    }
}

fn ilp_cli<'a, 'b>() -> App<'a, 'b> {
    App::new("ilp-cli")
        .about("Interledger.rs Command-Line Interface")
        .version(crate_version!())
        .global_settings(&[
            AppSettings::AllowNegativeNumbers,
            AppSettings::VersionlessSubcommands,
            //AppSettings::SubcommandRequiredElseHelp, // TODO: re-enable this
        ])
        // TODO remove this line once this issue is solved:
        // https://github.com/clap-rs/clap/issues/1536
        .after_help("")
        .args(&[
            Arg::with_name("node_url")
                .long("node")
                .env("ILP_CLI_NODE_URL")
                .default_value("http://localhost:7770")
                .help("The base URL of the node to connect to"),
            Arg::with_name("quiet")
                .short("q")
                .long("quiet")
                .help("Disable printing the bodies of successful HTTP responses upon receipt"),
        ])
}

fn accounts<'a, 'b>() -> App<'a, 'b> {
    SubCommand::with_name("accounts").about("Operations for interacting with accounts")
}

fn accounts_balance<'a, 'b>() -> App<'a, 'b> {
    AuthorizedSubCommand::with_name("balance")
        .about("Returns the balance of an account")
        .arg(
            Arg::with_name("username")
                .index(1)
                .takes_value(true)
                .required(true)
                .help("The username of the account whose balance to return"),
        )
}

fn accounts_create<'a, 'b>() -> App<'a, 'b> {
    AuthorizedSubCommand::with_name("create")
        .about("Creates a new account on this node")
        .args(&[
            Arg::with_name("username")
                .index(1)
                .takes_value(true)
                .required(true)
                .help("The username of the account"),
            Arg::with_name("asset_code")
                .long("asset-code")
                .takes_value(true)
                .required(true)
                .help("The code of the asset associated with this account"),
            Arg::with_name("asset_scale")
                .long("asset-scale")
                .takes_value(true)
                .required(true)
                .help("The scale of the asset associated with this account"),
            // TODO: when we have a glossary of HTTP API options, add their descriptions to these
            Arg::with_name("ilp_address")
                .long("ilp-address")
                .takes_value(true),
            Arg::with_name("max_packet_amount")
                .long("max-packet-amount")
                .takes_value(true),
            Arg::with_name("min_balance")
                .long("min-balance")
                .takes_value(true),
            Arg::with_name("ilp_over_http_url")
                .long("ilp-over-http-url")
                .takes_value(true),
            Arg::with_name("ilp_over_http_incoming_token")
                .long("ilp-over-http-incoming-token")
                .takes_value(true),
            Arg::with_name("ilp_over_http_outgoing_token")
                .long("ilp-over-http-outgoing-token")
                .takes_value(true),
            Arg::with_name("ilp_over_btp_url")
                .long("ilp-over-btp-url")
                .takes_value(true),
            Arg::with_name("ilp_over_btp_outgoing_token")
                .long("ilp-over-btp-outgoing-token")
                .takes_value(true),
            Arg::with_name("ilp_over_btp_incoming_token")
                .long("ilp-over-btp-incoming-token")
                .takes_value(true),
            Arg::with_name("settle_threshold")
                .long("settle-threshold")
                .takes_value(true),
            Arg::with_name("settle_to")
                .long("settle-to")
                .takes_value(true),
            Arg::with_name("routing_relation")
                .long("routing-relation")
                .takes_value(true),
            Arg::with_name("round_trip_time")
                .long("round-trip-time")
                .takes_value(true),
            Arg::with_name("amount_per_minute_limit")
                .long("amount-per-minute-limit")
                .takes_value(true),
            Arg::with_name("packets_per_minute_limit")
                .long("packets-per-minute-limit")
                .takes_value(true),
            Arg::with_name("settlement_engine_url")
                .long("settlement-engine-url")
                .takes_value(true),
        ])
}

fn accounts_update<'a, 'b>() -> App<'a, 'b> {
    AuthorizedSubCommand::with_name("update")
        .about("Creates a new account on this node")
        .args(&[
            Arg::with_name("username")
                .index(1)
                .takes_value(true)
                .required(true)
                .help("The username of the account"),
            Arg::with_name("asset_code")
                .long("asset-code")
                .takes_value(true)
                .required(true)
                .help("The code of the asset associated with this account"),
            Arg::with_name("asset_scale")
                .long("asset-scale")
                .takes_value(true)
                .required(true)
                .help("The scale of the asset associated with this account"),
            // TODO: when we have a glossary of HTTP API options, add their descriptions to these
            Arg::with_name("ilp_address")
                .long("ilp-address")
                .takes_value(true),
            Arg::with_name("max_packet_amount")
                .long("max-packet-amount")
                .takes_value(true),
            Arg::with_name("min_balance")
                .long("min-balance")
                .takes_value(true),
            Arg::with_name("ilp_over_http_url")
                .long("ilp-over-http-url")
                .takes_value(true),
            Arg::with_name("ilp_over_http_incoming_token")
                .long("ilp-over-http-incoming-token")
                .takes_value(true),
            Arg::with_name("ilp_over_http_outgoing_token")
                .long("ilp-over-http-outgoing-token")
                .takes_value(true),
            Arg::with_name("ilp_over_btp_url")
                .long("ilp-over-btp-url")
                .takes_value(true),
            Arg::with_name("ilp_over_btp_outgoing_token")
                .long("ilp-over-btp-outgoing-token")
                .takes_value(true),
            Arg::with_name("ilp_over_btp_incoming_token")
                .long("ilp-over-btp-incoming-token")
                .takes_value(true),
            Arg::with_name("settle_threshold")
                .long("settle-threshold")
                .takes_value(true),
            Arg::with_name("settle_to")
                .long("settle-to")
                .takes_value(true),
            Arg::with_name("routing_relation")
                .long("routing-relation")
                .takes_value(true),
            Arg::with_name("round_trip_time")
                .long("round-trip-time")
                .takes_value(true),
            Arg::with_name("amount_per_minute_limit")
                .long("amount-per-minute-limit")
                .takes_value(true),
            Arg::with_name("packets_per_minute_limit")
                .long("packets-per-minute-limit")
                .takes_value(true),
            Arg::with_name("settlement_engine_url")
                .long("settlement-engine-url")
                .takes_value(true),
        ])
}

fn accounts_delete<'a, 'b>() -> App<'a, 'b> {
    AuthorizedSubCommand::with_name("delete")
        .about("Delete the given account")
        .arg(
            Arg::with_name("username")
                .index(1)
                .takes_value(true)
                .required(true)
                .help("The username of the account to delete"),
        )
}

fn accounts_incoming_payments<'a, 'b>() -> App<'a, 'b> {
    AuthorizedSubCommand::with_name("incoming-payments")
        .about("Open a persistent connection to a node for monitoring incoming payments to an account [COMING SOON]")
        .about(
            "Open a persistent connection to a node for monitoring incoming payments to an account",
        )
        .arg(
            Arg::with_name("username")
                .index(1)
                .takes_value(true)
                .required(true)
                .help("The username of the account to monitor"),
        )
}

fn accounts_info<'a, 'b>() -> App<'a, 'b> {
    AuthorizedSubCommand::with_name("info")
        .about("View details of a given account")
        .arg(
            Arg::with_name("username")
                .index(1)
                .takes_value(true)
                .required(true)
                .help("The username of the account to view"),
        )
}

fn accounts_list<'a, 'b>() -> App<'a, 'b> {
    AuthorizedSubCommand::with_name("list").about("List all accounts on this node")
}

fn accounts_update_settings<'a, 'b>() -> App<'a, 'b> {
    AuthorizedSubCommand::with_name("update-settings")
        .about("Overwrite the details of an account on this node")
        .args(&[
            Arg::with_name("username")
                .index(1)
                .takes_value(true)
                .required(true)
                .help("The username of the account"),
            Arg::with_name("ilp_over_http_url")
                .long("ilp-over-http-url")
                .takes_value(true),
            Arg::with_name("ilp_over_http_incoming_token")
                .long("ilp-over-http-incoming-token")
                .takes_value(true),
            Arg::with_name("ilp_over_http_outgoing_token")
                .long("ilp-over-http-outgoing-token")
                .takes_value(true),
            Arg::with_name("ilp_over_btp_url")
                .long("ilp-over-btp-url")
                .takes_value(true),
            Arg::with_name("ilp_over_btp_outgoing_token")
                .long("ilp-over-btp-outgoing-token")
                .takes_value(true),
            Arg::with_name("ilp_over_btp_incoming_token")
                .long("ilp-over-btp-incoming-token")
                .takes_value(true),
            Arg::with_name("settle_threshold")
                .long("settle-threshold")
                .takes_value(true),
            Arg::with_name("settle_to")
                .long("settle-to")
                .takes_value(true),
        ])
}

fn pay<'a, 'b>() -> App<'a, 'b> {
    // TODO: this endpoint currently only works with user authorization, not admin authorization
    AuthorizedSubCommand::with_name("pay")
        .about("Send a payment from an account on this node")
        .args(&[
            Arg::with_name("sender_username")
                .index(1)
                .takes_value(true)
                .required(true)
                .help("The username of the account on this node issuing the payment"),
            Arg::with_name("source_amount")
                .long("amount")
                .takes_value(true)
                .required(true)
                .help("The amount to transfer from the sender to the receiver, denominated in units of the sender's assets"),
            Arg::with_name("receiver")
                .long("to")
                .takes_value(true)
                .required(true)
                .help("The Payment Pointer or SPSP address of the account receiving the payment"),
        ])
}

fn rates<'a, 'b>() -> App<'a, 'b> {
    SubCommand::with_name("rates").about("Operations for interacting with exchange rates")
}

fn rates_list<'a, 'b>() -> App<'a, 'b> {
    SubCommand::with_name("list").about("List the current exchange rates known to this node")
}

fn rates_set_all<'a, 'b>() -> App<'a, 'b> {
    AuthorizedSubCommand::with_name("set-all")
        .about("Overwrite the list of exchange rates used by this node")
        .arg(
            Arg::with_name("halve")
                .long("pair")
                .number_of_values(2)
                .multiple(true)
                .help("A set of space-separated key/value pairs, representing an asset code and an exchange rate; may appear multiple times"),
        )
}

fn routes<'a, 'b>() -> App<'a, 'b> {
    SubCommand::with_name("routes").about("Operations for interacting with the routing table")
}

fn routes_list<'a, 'b>() -> App<'a, 'b> {
    SubCommand::with_name("list").about("View this node's routing table")
}

fn routes_set<'a, 'b>() -> App<'a, 'b> {
    AuthorizedSubCommand::with_name("set")
        .about("Configure a single static route on this node")
        .args(&[
            Arg::with_name("prefix")
                .index(1)
                .takes_value(true)
                .required(true)
                .help("The routing prefix to configure"),
            Arg::with_name("destination")
                .long("destination")
                .takes_value(true)
                .required(true)
                .help("The destination to associate with the provided prefix"),
        ])
}

fn routes_set_all<'a, 'b>() -> App<'a, 'b> {
    AuthorizedSubCommand::with_name("set-all")
        .about("Overwrite the list of static routes used by this node")
        .arg(
            Arg::with_name("halve")
                .long("pair")
                .number_of_values(2)
                .multiple(true)
                .help("A set of space-separated key/value pairs, representing a route and its destination; may appear multiple times"),
        )
}

fn settlement_engines<'a, 'b>() -> App<'a, 'b> {
    SubCommand::with_name("settlement-engines")
        .about("Interact with the settlement engine configurations")
}

fn settlement_engines_set_all<'a, 'b>() -> App<'a, 'b> {
    AuthorizedSubCommand::with_name("set-all")
        .about("Configure the default settlement engines for given asset codes")
        .arg(
            Arg::with_name("halve")
                .long("pair")
                .number_of_values(2)
                .multiple(true)
                .help("A set of space-separated key/value pairs, representing an asset code and a settlement engine; may appear multiple times"),
        )
}

fn status<'a, 'b>() -> App<'a, 'b> {
    SubCommand::with_name("status").about("Query the status of the server")
}

fn testnet<'a, 'b>() -> App<'a, 'b> {
    SubCommand::with_name("testnet").about("Easily access the testnet")
}

fn testnet_setup<'a, 'b>() -> App<'a, 'b> {
    AuthorizedSubCommand::with_name("setup")
        .about("Create a local account peered with a remote node on the testnet")
        .args(&[
            Arg::with_name("asset")
                .index(1)
                .required(true)
                .takes_value(true)
                .possible_values(&["xrp", "eth"])
                .case_insensitive(true)
                .help("The asset that will be tied to the new testnet account"),
            Arg::with_name("return_testnet_credential")
                .long("return-testnet-credential")
                .help("Return the authorization credential for our account on the testnet node instead of the account on our local node"),
        ])
}
