#![allow(unused_variables)]

use clap::ArgMatches;
use reqwest::{self, Client, Response};
use std::collections::HashMap;

pub enum Error {
    UsageErr(&'static str),
    ClientErr(reqwest::Error),
}

pub fn run<'a, 'b>(matches: &ArgMatches) -> Result<Response, Error> {
    let client = NodeClient {
        client: Client::new(),
        // `--node` has a a default value, so will never be None
        url: matches.value_of("node_url").unwrap(),
    };

    // Dispatch based on parsed input
    match matches.subcommand() {
        // Execute the specified subcommand
        (ilp_cli_subcommand, Some(ilp_cli_matches)) => {
            // Send HTTP request
            match ilp_cli_subcommand {
                "accounts" => match ilp_cli_matches.subcommand() {
                    (accounts_subcommand, Some(accounts_matches)) => match accounts_subcommand {
                        "balance" => client.get_account_balance(accounts_matches),
                        "create" => client.post_accounts(accounts_matches),
                        "delete" => client.delete_account(accounts_matches),
                        "incoming-payments" => {
                            client.ws_account_payments_incoming(accounts_matches)
                        }
                        "info" => client.get_account(accounts_matches),
                        "list" => client.get_accounts(accounts_matches),
                        "update" => {
                            if accounts_matches.is_present("is_admin") {
                                client.put_account(accounts_matches)
                            } else {
                                client.put_account_settings(accounts_matches)
                            }
                        }
                        command => panic!("Unhandled `ilp-cli accounts` subcommand: {}", command),
                    },
                    _ => Err(Error::UsageErr("ilp-cli help accounts")),
                },
                "pay" => client.post_account_payments(ilp_cli_matches),
                "rates" => match ilp_cli_matches.subcommand() {
                    (rates_subcommand, Some(rates_matches)) => match rates_subcommand {
                        "list" => client.get_rates(rates_matches),
                        "set-all" => client.put_rates(rates_matches),
                        command => panic!("Unhandled `ilp-cli rates` subcommand: {}", command),
                    },
                    _ => Err(Error::UsageErr("ilp-cli help rates")),
                },
                "routes" => match ilp_cli_matches.subcommand() {
                    (routes_subcommand, Some(routes_matches)) => match routes_subcommand {
                        "list" => client.get_routes(routes_matches),
                        "set" => client.put_route_static(routes_matches),
                        "set-all" => client.put_routes_static(routes_matches),
                        command => panic!("Unhandled `ilp-cli routes` subcommand: {}", command),
                    },
                    _ => Err(Error::UsageErr("ilp-cli help routes")),
                },
                "settlement-engines" => match ilp_cli_matches.subcommand() {
                    (settlement_engines_subcommand, Some(settlement_engines_matches)) => {
                        match settlement_engines_subcommand {
                            "set-all" => client.put_settlement_engines(ilp_cli_matches),
                            command => panic!(
                                "Unhandled `ilp-cli settlement-engines` subcommand: {}",
                                command
                            ),
                        }
                    }
                    _ => Err(Error::UsageErr("ilp-cli help settlement-engines")),
                },
                "status" => client.get_root(ilp_cli_matches),
                command => panic!("Unhandled `ilp-cli` subcommand: {}", command),
            }
        }
        _ => Err(Error::UsageErr("ilp-cli help")),
    }
}

struct NodeClient<'a> {
    client: Client,
    url: &'a str,
}

impl NodeClient<'_> {
    fn get_account_balance(&self, matches: &ArgMatches) -> Result<Response, Error> {
        let (auth, mut args) = extract_args(matches);
        let user = args.remove("account_username").unwrap();
        self.client
            .get(&format!("{}/accounts/{}/balance", self.url, user))
            .bearer_auth(auth)
            .send()
            .map_err(Error::ClientErr)
    }

    fn post_accounts(&self, matches: &ArgMatches) -> Result<Response, Error> {
        let (auth, args) = extract_args(matches);
        self.client
            .post(&format!("{}/accounts", self.url))
            .bearer_auth(auth)
            .json(&args)
            .send()
            .map_err(Error::ClientErr)
    }

    fn delete_account(&self, matches: &ArgMatches) -> Result<Response, Error> {
        let (auth, args) = extract_args(matches);
        self.client
            .delete(&format!("{}/accounts/{}", self.url, args["username"]))
            .bearer_auth(auth)
            .send()
            .map_err(Error::ClientErr)
    }

    fn ws_account_payments_incoming(&self, matches: &ArgMatches) -> Result<Response, Error> {
        unimplemented!()
    }

    fn get_account(&self, matches: &ArgMatches) -> Result<Response, Error> {
        let (auth, args) = extract_args(matches);
        self.client
            .get(&format!("{}/accounts/{}", self.url, args["username"]))
            .bearer_auth(auth)
            .send()
            .map_err(Error::ClientErr)
    }

    fn get_accounts(&self, matches: &ArgMatches) -> Result<Response, Error> {
        let (auth, _) = extract_args(matches);
        self.client
            .get(&format!("{}/accounts", self.url))
            .bearer_auth(auth)
            .send()
            .map_err(Error::ClientErr)
    }

    fn put_account(&self, matches: &ArgMatches) -> Result<Response, Error> {
        let (auth, mut args) = extract_args(matches);
        let user = args.remove("username").unwrap();
        self.client
            .put(&format!("{}/accounts/{}", self.url, user))
            .bearer_auth(auth)
            .json(&args)
            .send()
            .map_err(Error::ClientErr)
    }

    fn put_account_settings(&self, matches: &ArgMatches) -> Result<Response, Error> {
        let (auth, mut args) = extract_args(matches);
        let user = args.remove("username").unwrap();
        self.client
            .put(&format!("{}/accounts/{}/settings", self.url, user))
            .bearer_auth(auth)
            .json(&args)
            .send()
            .map_err(Error::ClientErr)
    }

    fn post_account_payments(&self, matches: &ArgMatches) -> Result<Response, Error> {
        let (auth, mut args) = extract_args(matches);
        let user = args.remove("sender_username").unwrap();
        self.client
            .post(&format!("{}/accounts/{}/payments", self.url, user))
            .bearer_auth(&format!("{}:{}", user, auth))
            .json(&args)
            .send()
            .map_err(Error::ClientErr)
    }

    fn get_rates(&self, matches: &ArgMatches) -> Result<Response, Error> {
        self.client
            .get(&format!("{}/rates", self.url))
            .send()
            .map_err(Error::ClientErr)
    }

    fn put_rates(&self, matches: &ArgMatches) -> Result<Response, Error> {
        let auth = matches.value_of("authorization_key").unwrap();
        let mut rates = HashMap::new();
        if let Some(rate_matches) = matches.values_of("rate") {
            let rate_matches: Vec<&str> = rate_matches.collect();
            let codes: Vec<String> = rate_matches
                .iter()
                .enumerate()
                .filter(|(i, _)| i % 2 == 0)
                .map(|(_, code)| code.to_string())
                .collect();
            let values: Vec<f64> = rate_matches
                .iter()
                .enumerate()
                .filter(|(i, _)| i % 2 == 1)
                .map(|(_, value)| value.parse().unwrap())
                .collect();
            for (code, value) in codes.into_iter().zip(values.into_iter()) {
                rates.insert(code, value);
            }
        }
        self.client
            .put(&format!("{}/rates", self.url))
            .bearer_auth(auth)
            .json(&rates)
            .send()
            .map_err(Error::ClientErr)
    }

    fn get_routes(&self, matches: &ArgMatches) -> Result<Response, Error> {
        self.client
            .get(&format!("{}/routes", self.url))
            .send()
            .map_err(Error::ClientErr)
    }

    fn put_route_static(&self, matches: &ArgMatches) -> Result<Response, Error> {
        unimplemented!()
    }

    fn put_routes_static(&self, matches: &ArgMatches) -> Result<Response, Error> {
        let auth = matches.value_of("authorization_key").unwrap();
        let mut combined_routes = HashMap::new();
        if let Some(route_matches) = matches.values_of("route") {
            let route_matches: Vec<&str> = route_matches.collect();
            let prefixes: Vec<String> = route_matches
                .iter()
                .enumerate()
                .filter(|(i, _)| i % 2 == 0)
                .map(|(_, prefix)| prefix.to_string())
                .collect();
            let routes: Vec<String> = route_matches
                .iter()
                .enumerate()
                .filter(|(i, _)| i % 2 == 1)
                .map(|(_, route)| route.to_string())
                .collect();
            for (prefix, route) in prefixes.into_iter().zip(routes.into_iter()) {
                combined_routes.insert(prefix, route);
            }
        }
        self.client
            .put(&format!("{}/routes/static", self.url))
            .bearer_auth(auth)
            .json(&combined_routes)
            .send()
            .map_err(Error::ClientErr)
    }

    fn put_settlement_engines(&self, matches: &ArgMatches) -> Result<Response, Error> {
        unimplemented!()
    }

    fn get_root(&self, matches: &ArgMatches) -> Result<Response, Error> {
        self.client
            .get(&format!("{}/", self.url))
            .send()
            .map_err(Error::ClientErr)
    }
}

// This function takes the map of arguments parsed by Clap
// and extracts the values for each argument.
fn extract_args<'a>(matches: &'a ArgMatches) -> (&'a str, HashMap<&'a str, &'a str>) {
    let mut args: HashMap<_, _> = matches // Contains data and metadata about the parsed command
        .args // The hashmap containing each parameter along with its values and metadata
        .iter()
        .map(|(&key, val)| (key, val.vals[0].to_str().unwrap())) // Extract raw key/value pairs
        .collect();
    let auth = args.remove("authorization_key").unwrap();
    (auth, args)
}
