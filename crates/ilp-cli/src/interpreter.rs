#![allow(unused_variables)]

use clap::{App, ArgMatches};
use reqwest::{Client, Response};
use std::process::exit;

pub fn run<'a, 'b>(mut app: App<'a, 'b>) {
    // Parse the CLI input using the defined arguments
    let matches = app.clone().get_matches();

    let node = Node {
        client: Client::new(),
        // `--auth` is a required argument, so will never be None
        auth: matches.value_of("authorization_key").unwrap(),
        // `--node` has a a default value, so will never be None
        url: matches.value_of("node_url").unwrap(),
    };

    // Dispatch based on parsed input
    match matches.subcommand() {
        // Execute the specified subcommand
        (ilp_cli_subcommand, Some(ilp_cli_matches)) => {
            // Send HTTP request
            let response = match ilp_cli_subcommand {
                "accounts" => match ilp_cli_matches.subcommand() {
                    (accounts_subcommand, Some(accounts_matches)) => match accounts_subcommand {
                        "balance" => node.get_account_balance(accounts_matches),
                        "create" => node.post_accounts(accounts_matches),
                        "delete" => node.delete_account(accounts_matches),
                        "incoming-payments" => node.ws_account_payments_incoming(accounts_matches),
                        "info" => node.get_account(accounts_matches),
                        "list" => node.get_accounts(accounts_matches),
                        "update" => {
                            if accounts_matches.is_present("is_admin") {
                                node.put_account(accounts_matches)
                            } else {
                                node.put_account_settings(accounts_matches)
                            }
                        }
                        command => panic!("Unhandled `ilp-cli accounts` subcommand: {}", command),
                    },
                    // TODO: subcommand-specific help output
                    _ => {
                        app.print_help().unwrap();
                        exit(1);
                    }
                },
                "pay" => node.post_account_payments(ilp_cli_matches),
                "rates" => match ilp_cli_matches.subcommand() {
                    (rates_subcommand, Some(rates_matches)) => match rates_subcommand {
                        "list" => node.get_rates(rates_matches),
                        "set-all" => node.put_rates(rates_matches),
                        command => panic!("Unhandled `ilp-cli rates` subcommand: {}", command),
                    },
                    _ => {
                        app.print_help().unwrap();
                        exit(1);
                    }
                },
                "routes" => match ilp_cli_matches.subcommand() {
                    (routes_subcommand, Some(routes_matches)) => match routes_subcommand {
                        "list" => node.get_routes(routes_matches),
                        "set" => node.put_route_static(routes_matches),
                        "set-all" => node.put_routes_static(routes_matches),
                        command => panic!("Unhandled `ilp-cli routes` subcommand: {}", command),
                    },
                    _ => {
                        app.print_help().unwrap();
                        exit(1);
                    }
                },
                "settlement-engines" => match ilp_cli_matches.subcommand() {
                    (settlement_engines_subcommand, Some(settlement_engines_matches)) => {
                        match settlement_engines_subcommand {
                            "set-all" => node.put_settlement_engines(settlement_engines_matches),
                            command => panic!(
                                "Unhandled `ilp-cli settlement-engines` subcommand: {}",
                                command
                            ),
                        }
                    }
                    _ => {
                        app.print_help().unwrap();
                        exit(1);
                    }
                },
                "status" => node.get_root(ilp_cli_matches),
                command => panic!("Unhandled `ilp-cli` subcommand: {}", command),
            };

            // Handle HTTP response
            match response {
                Err(e) => {
                    eprintln!("ILP CLI error: failed to send request: {}", e);
                    exit(1);
                }
                Ok(mut res) => match res.text() {
                    Err(e) => {
                        eprintln!("ILP CLI error: failed to parse response: {}", e);
                        exit(1);
                    }
                    // Final output
                    Ok(val) => {
                        if res.status().is_success() {
                            if !matches.is_present("quiet") {
                                println!("{}", val)
                            }
                        } else {
                            eprintln!(
                                "ILP CLI error: unsuccessful response from node: {}: {}",
                                res.status(),
                                val
                            );
                            exit(1);
                        }
                    }
                },
            }
        }
        // No subcommand identified within parsed input
        _ => {
            app.print_help().unwrap();
            exit(1);
        }
    }
}

struct Node<'a> {
    client: Client,
    auth: &'a str,
    url: &'a str,
}

impl Node<'_> {
    fn get_account_balance(&self, matches: &ArgMatches) -> reqwest::Result<Response> {
        let user = matches.value_of("account_username").unwrap();
        self.client
            .get(&format!("{}/accounts/{}/balance", self.url, user))
            .bearer_auth(self.auth)
            .send()
    }

    fn post_accounts(&self, matches: &ArgMatches) -> reqwest::Result<Response> {
        let args = extract_args(matches);
        self.client
            .post(&format!("{}/accounts", self.url))
            .bearer_auth(self.auth)
            .json(&args)
            .send()
    }

    fn delete_account(&self, matches: &ArgMatches) -> reqwest::Result<Response> {
        unimplemented!()
    }

    fn ws_account_payments_incoming(&self, matches: &ArgMatches) -> reqwest::Result<Response> {
        unimplemented!()
    }

    fn get_account(&self, matches: &ArgMatches) -> reqwest::Result<Response> {
        unimplemented!()
    }

    fn get_accounts(&self, matches: &ArgMatches) -> reqwest::Result<Response> {
        unimplemented!()
    }

    fn put_account(&self, matches: &ArgMatches) -> reqwest::Result<Response> {
        unimplemented!()
    }

    fn put_account_settings(&self, matches: &ArgMatches) -> reqwest::Result<Response> {
        unimplemented!()
    }

    fn post_account_payments(&self, matches: &ArgMatches) -> reqwest::Result<Response> {
        let mut args = extract_args(matches);
        let user = args.remove("sender_username").unwrap();
        self.client
            .post(&format!("{}/accounts/{}/payments", self.url, user))
            .bearer_auth(&format!("{}:{}", user, self.auth))
            .json(&args)
            .send()
    }

    fn get_rates(&self, matches: &ArgMatches) -> reqwest::Result<Response> {
        unimplemented!()
    }

    fn put_rates(&self, matches: &ArgMatches) -> reqwest::Result<Response> {
        unimplemented!()
    }

    fn get_routes(&self, matches: &ArgMatches) -> reqwest::Result<Response> {
        unimplemented!()
    }

    fn put_route_static(&self, matches: &ArgMatches) -> reqwest::Result<Response> {
        unimplemented!()
    }

    fn put_routes_static(&self, matches: &ArgMatches) -> reqwest::Result<Response> {
        unimplemented!()
    }

    fn put_settlement_engines(&self, matches: &ArgMatches) -> reqwest::Result<Response> {
        unimplemented!()
    }

    fn get_root(&self, matches: &ArgMatches) -> reqwest::Result<Response> {
        unimplemented!()
    }
}

// This function takes the map of arguments parsed by Clap
// and extracts the values for each argument.
fn extract_args<'a>(matches: &'a ArgMatches) -> std::collections::HashMap<&'a str, &'a str> {
    matches // Contains data and metadata about the parsed command
        .args // The hashmap containing each parameter along with its values and metadata
        .iter()
        .map(|(&key, val)| (key, val.vals[0].to_str().unwrap())) // Extract raw key/value pairs
        .collect()
}
