use clap::{crate_version, App, Arg, SubCommand};

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
        ]),
        pay(),
        rates().subcommands(vec![rates_list(), rates_set_all()]),
        routes().subcommands(vec![routes_list(), routes_set(), routes_set_all()]),
        settlement_engines().subcommands(vec![settlement_engines_set_all()]),
        status(),
    ])
}

fn ilp_cli<'a, 'b>() -> App<'a, 'b> {
    App::new("ilp-cli")
        .about("Interledger.rs Command-Line Interface")
        .version(crate_version!())
        // TODO remove this line once this issue is solved:
        // https://github.com/clap-rs/clap/issues/1536
        .after_help("")
        .args(&[
            Arg::with_name("authorization_key")
                .long("auth")
                .env("ILP_CLI_API_AUTH")
                .required(true)
                .help("An HTTP bearer authorization token permitting access to the designated operation"),
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
    SubCommand::with_name("accounts")
        .about("Operations for interacting with accounts")
        .arg(
            Arg::with_name("is_admin")
                .long("admin")
                .help("Attempts to perform the specified operation as an administrator"),
        )
}

fn accounts_balance<'a, 'b>() -> App<'a, 'b> {
    // Example: ilp-cli get-balance alice
    SubCommand::with_name("balance")
        .about("Returns the balance of an account")
        .arg(
            Arg::with_name("account_username")
                .index(1)
                .takes_value(true)
                .required(true)
                .help("The username of the account whose balance to return"),
        )
}

fn accounts_create<'a, 'b>() -> App<'a, 'b> {
    // Example: ilp-cli add-account alice ABC 9
    SubCommand::with_name("create")
        .about("Creates a new account on this node")
        .args(&[
            Arg::with_name("username")
                .index(1)
                .takes_value(true)
                .required(true)
                .help("The username of the new account"),
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
                .takes_value(true)
                .allow_hyphen_values(true),
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
                .takes_value(true)
                .allow_hyphen_values(true),
            Arg::with_name("settle_to")
                .long("settle-to")
                .takes_value(true)
                .allow_hyphen_values(true),
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
    SubCommand::with_name("delete")
}

fn accounts_incoming_payments<'a, 'b>() -> App<'a, 'b> {
    SubCommand::with_name("incoming-payments")
}

fn accounts_info<'a, 'b>() -> App<'a, 'b> {
    SubCommand::with_name("info")
}

fn accounts_list<'a, 'b>() -> App<'a, 'b> {
    SubCommand::with_name("list")
}

fn accounts_update<'a, 'b>() -> App<'a, 'b> {
    SubCommand::with_name("update")
}

fn pay<'a, 'b>() -> App<'a, 'b> {
    // Example: ilp-cli post-payment alice 500 "http://localhost:8770/accounts/bob/spsp"
    // TODO: this currently only works with user authorization, not admin authorization
    SubCommand::with_name("pay")
                .about("Send a payment from an account on this node")
                .args(&[
                    Arg::with_name("sender_username")
                        .index(1)
                        .takes_value(true)
                        .required(true)
                        .help("The username of the account on this node issuing the payment"),
                    Arg::with_name("source_amount")
                        .long("source-amount")
                        .takes_value(true)
                        .required(true)
                        .help("The amount to transfer from the sender to the receiver, denominated in units of the sender's assets"),
                    Arg::with_name("receiver")
                        .long("receiver")
                        .takes_value(true)
                        .required(true)
                        .help("The Payment Pointer or SPSP address of the account receiving the payment"),
                ])
}

fn rates<'a, 'b>() -> App<'a, 'b> {
    SubCommand::with_name("rates")
}

fn rates_list<'a, 'b>() -> App<'a, 'b> {
    SubCommand::with_name("list")
}

fn rates_set_all<'a, 'b>() -> App<'a, 'b> {
    SubCommand::with_name("set-all")
}

fn routes<'a, 'b>() -> App<'a, 'b> {
    SubCommand::with_name("routes")
}

fn routes_list<'a, 'b>() -> App<'a, 'b> {
    SubCommand::with_name("list")
}

fn routes_set<'a, 'b>() -> App<'a, 'b> {
    SubCommand::with_name("set")
}

fn routes_set_all<'a, 'b>() -> App<'a, 'b> {
    SubCommand::with_name("set-all")
}

fn settlement_engines<'a, 'b>() -> App<'a, 'b> {
    SubCommand::with_name("settlement-engines")
}

fn settlement_engines_set_all<'a, 'b>() -> App<'a, 'b> {
    SubCommand::with_name("set-all")
}

fn status<'a, 'b>() -> App<'a, 'b> {
    SubCommand::with_name("status")
}
