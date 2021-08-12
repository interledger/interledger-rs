use clap::ArgMatches;
use reqwest::{
    self,
    blocking::{Client, Response},
};
use std::collections::HashMap;
use tokio_tungstenite::tungstenite::{connect, handshake::client::Request};
use url::Url;

#[non_exhaustive]
#[derive(Debug, thiserror::Error)]
pub enum Error {
    // Custom errors
    #[error("Usage error")]
    Usage(&'static str),
    #[error("Invalid protocol in URL: {0}")]
    Protocol(String),
    // Foreign errors
    #[error("Error sending HTTP request: {0}")]
    Send(#[from] reqwest::Error),
    #[error("Error receving HTTP response from testnet: {0}")]
    Testnet(reqwest::Error),
    #[error("Error altering URL scheme")]
    Scheme(()), // TODO: should be part of UrlError, see https://github.com/servo/rust-url/issues/299
    #[error("Error parsing URL: {0}")]
    Url(#[from] url::ParseError),
    #[error("WebSocket error: {0}")]
    WebsocketErr(#[from] tokio_tungstenite::tungstenite::error::Error),
    #[error("HTTP error: {0}")]
    Http(#[from] http::Error),
}

pub fn run(matches: &ArgMatches) -> Result<Response, Error> {
    let client = NodeClient {
        client: Client::new(),
        url: matches.value_of("node_url").unwrap(), // infallible unwrap
    };

    // Dispatch based on parsed input
    match matches.subcommand() {
        ("accounts", Some(accounts_matches)) => match accounts_matches.subcommand() {
            ("balance", Some(submatches)) => client.get_account_balance(submatches),
            ("create", Some(submatches)) => client.post_accounts(submatches),
            ("delete", Some(submatches)) => client.delete_account(submatches),
            ("incoming-payments", Some(submatches)) => {
                client.ws_account_payments_incoming(submatches)
            }
            ("info", Some(submatches)) => client.get_account(submatches),
            ("list", Some(submatches)) => client.get_accounts(submatches),
            ("update", Some(submatches)) => client.put_account(submatches),
            ("update-settings", Some(submatches)) => client.put_account_settings(submatches),
            _ => Err(Error::Usage("ilp-cli help accounts")),
        },
        ("pay", Some(pay_matches)) => client.post_account_payments(pay_matches),
        ("rates", Some(rates_matches)) => match rates_matches.subcommand() {
            ("list", Some(submatches)) => client.get_rates(submatches),
            ("set-all", Some(submatches)) => client.put_rates(submatches),
            _ => Err(Error::Usage("ilp-cli help rates")),
        },
        ("routes", Some(routes_matches)) => match routes_matches.subcommand() {
            ("list", Some(submatches)) => client.get_routes(submatches),
            ("set", Some(submatches)) => client.put_route_static(submatches),
            ("set-all", Some(submatches)) => client.put_routes_static(submatches),
            _ => Err(Error::Usage("ilp-cli help routes")),
        },
        ("settlement-engines", Some(settlement_matches)) => match settlement_matches.subcommand() {
            ("set-all", Some(submatches)) => client.put_settlement_engines(submatches),
            _ => Err(Error::Usage("ilp-cli help settlement-engines")),
        },
        ("status", Some(status_matches)) => client.get_root(status_matches),
        ("logs", Some(log_level)) => client.put_tracing_level(log_level),
        ("testnet", Some(testnet_matches)) => match testnet_matches.subcommand() {
            ("setup", Some(submatches)) => client.xpring_account(submatches),
            _ => Err(Error::Usage("ilp-cli help testnet")),
        },
        ("payments", Some(payments_matches)) => match payments_matches.subcommand() {
            ("incoming", Some(submatches)) => client.ws_payments_incoming(submatches),
            _ => Err(Error::Usage("ilp-cli help payments")),
        },
        _ => Err(Error::Usage("ilp-cli help")),
    }
}

struct NodeClient<'a> {
    client: Client,
    url: &'a str,
}

impl NodeClient<'_> {
    // GET /accounts/:username/balance
    fn get_account_balance(&self, matches: &ArgMatches) -> Result<Response, Error> {
        let (auth, mut args) = extract_args(matches);
        let user = args.remove("username").unwrap(); // infallible unwrap
        self.client
            .get(&format!("{}/accounts/{}/balance", self.url, user))
            .bearer_auth(auth)
            .send()
            .map_err(Error::Send)
    }

    // POST /accounts
    fn post_accounts(&self, matches: &ArgMatches) -> Result<Response, Error> {
        let (auth, args) = extract_args(matches);
        self.client
            .post(&format!("{}/accounts/", self.url))
            .bearer_auth(auth)
            .json(&args)
            .send()
            .map_err(Error::Send)
    }

    // PUT /accounts/:username
    fn put_account(&self, matches: &ArgMatches) -> Result<Response, Error> {
        let (auth, args) = extract_args(matches);
        self.client
            .put(&format!("{}/accounts/{}", self.url, args["username"]))
            .bearer_auth(auth)
            .json(&args)
            .send()
            .map_err(Error::Send)
    }

    // DELETE /accounts/:username
    fn delete_account(&self, matches: &ArgMatches) -> Result<Response, Error> {
        let (auth, args) = extract_args(matches);
        self.client
            .delete(&format!("{}/accounts/{}", self.url, args["username"]))
            .bearer_auth(auth)
            .send()
            .map_err(Error::Send)
    }

    // WebSocket /accounts/:username/payments/incoming
    fn ws_account_payments_incoming(&self, matches: &ArgMatches) -> Result<Response, Error> {
        let (auth, args) = extract_args(matches);
        let mut url = Url::parse(&format!(
            "{}/accounts/{}/payments/incoming",
            self.url, args["username"]
        ))?;

        let scheme = match url.scheme() {
            "http" => Ok("ws"),
            "https" => Ok("wss"),
            s => Err(Error::Protocol(format!(
                "{} (only HTTP and HTTPS are supported)",
                s
            ))),
        }?;

        url.set_scheme(scheme).map_err(Error::Scheme)?;

        let request: Request = Request::builder()
            .uri(url.into_string())
            .header("Authorization", format!("Bearer {}", auth))
            .body(())?;

        let (mut socket, _) = connect(request)?;
        loop {
            let msg = socket.read_message()?;
            println!("{}", msg);
        }
    }

    // WebSocket /payments/incoming
    fn ws_payments_incoming(&self, matches: &ArgMatches) -> Result<Response, Error> {
        let (auth, _args) = extract_args(matches);
        let mut url = Url::parse(&format!("{}/payments/incoming", self.url))?;

        let scheme = match url.scheme() {
            "http" => Ok("ws"),
            "https" => Ok("wss"),
            s => Err(Error::Protocol(format!(
                "{} (only HTTP and HTTPS are supported)",
                s
            ))),
        }?;

        url.set_scheme(scheme).map_err(Error::Scheme)?;

        let request: Request = Request::builder()
            .uri(url.into_string())
            .header("Authorization", format!("Bearer {}", auth))
            .body(())?;

        let (mut socket, _) = connect(request)?;
        loop {
            let msg = socket.read_message()?;
            println!("{}", msg);
        }
    }

    // GET /accounts/:username
    fn get_account(&self, matches: &ArgMatches) -> Result<Response, Error> {
        let (auth, args) = extract_args(matches);
        self.client
            .get(&format!("{}/accounts/{}", self.url, args["username"]))
            .bearer_auth(auth)
            .send()
            .map_err(Error::Send)
    }

    // GET /accounts
    fn get_accounts(&self, matches: &ArgMatches) -> Result<Response, Error> {
        let (auth, _) = extract_args(matches);
        self.client
            .get(&format!("{}/accounts", self.url))
            .bearer_auth(auth)
            .send()
            .map_err(Error::Send)
    }

    // PUT /accounts/:username/settings
    fn put_account_settings(&self, matches: &ArgMatches) -> Result<Response, Error> {
        let (auth, mut args) = extract_args(matches);
        let user = args.remove("username").unwrap(); // infallible unwrap
        self.client
            .put(&format!("{}/accounts/{}/settings", self.url, user))
            .bearer_auth(auth)
            .json(&args)
            .send()
            .map_err(Error::Send)
    }

    // POST /accounts/:username/payments
    fn post_account_payments(&self, matches: &ArgMatches) -> Result<Response, Error> {
        let (auth, mut args) = extract_args(matches);
        let user = args.remove("sender_username").unwrap(); // infallible unwrap
        self.client
            .post(&format!("{}/accounts/{}/payments", self.url, user))
            .bearer_auth(auth)
            .json(&args)
            .send()
            .map_err(Error::Send)
    }

    // GET /rates
    fn get_rates(&self, _matches: &ArgMatches) -> Result<Response, Error> {
        self.client
            .get(&format!("{}/rates", self.url))
            .send()
            .map_err(Error::Send)
    }

    // PUT /rates
    fn put_rates(&self, matches: &ArgMatches) -> Result<Response, Error> {
        let (auth, rate_pairs) = unflatten_pairs(matches);
        self.client
            .put(&format!("{}/rates", self.url))
            .bearer_auth(auth)
            .json(&rate_pairs)
            .send()
            .map_err(Error::Send)
    }

    // GET /routes
    fn get_routes(&self, _matches: &ArgMatches) -> Result<Response, Error> {
        self.client
            .get(&format!("{}/routes", self.url))
            .send()
            .map_err(Error::Send)
    }

    // PUT /routes/static/:prefix
    fn put_route_static(&self, matches: &ArgMatches) -> Result<Response, Error> {
        let (auth, args) = extract_args(matches);
        self.client
            .put(&format!("{}/routes/static/{}", self.url, args["prefix"]))
            .bearer_auth(auth)
            .body(args["destination"].to_string())
            .send()
            .map_err(Error::Send)
    }

    // PUT routes/static
    fn put_routes_static(&self, matches: &ArgMatches) -> Result<Response, Error> {
        let (auth, route_pairs) = unflatten_pairs(matches);
        self.client
            .put(&format!("{}/routes/static", self.url))
            .bearer_auth(auth)
            .json(&route_pairs)
            .send()
            .map_err(Error::Send)
    }

    // PUT /settlement/engines
    fn put_settlement_engines(&self, matches: &ArgMatches) -> Result<Response, Error> {
        let (auth, engine_pairs) = unflatten_pairs(matches);
        self.client
            .put(&format!("{}/settlement/engines", self.url))
            .bearer_auth(auth)
            .json(&engine_pairs)
            .send()
            .map_err(Error::Send)
    }

    // PUT /tracing-level
    fn put_tracing_level(&self, matches: &ArgMatches) -> Result<Response, Error> {
        let (auth, args) = extract_args(matches);
        self.client
            .put(&format!("{}/tracing-level", self.url))
            .bearer_auth(auth)
            .body(args["level"].to_owned())
            .send()
            .map_err(Error::Send)
    }

    // GET /
    fn get_root(&self, _matches: &ArgMatches) -> Result<Response, Error> {
        self.client
            .get(&format!("{}/", self.url))
            .send()
            .map_err(Error::Send)
    }

    /*
    {"http_endpoint": "https://rs3.xpring.dev/ilp", // ilp_over_http_url
    "passkey": "b0i3q9tbvfgek",  // ilp_over_http_outgoing_token = username:passkey
    "btp_endpoint": "btp+wss://rs3.xpring.dev/ilp/btp", // ilp_over_btp_url
    "asset_scale": 9, // asset_scale
    "node": "rs3.xpring.dev",
    "asset_code": "XRP",  // asset_code
    "username": "user_g31tuju4",  // username
    "payment_pointer": "$rs3.xpring.dev/accounts/user_g31tuju4/spsp"}
    routing_relation Parent
    */
    fn xpring_account(&self, matches: &ArgMatches) -> Result<Response, Error> {
        let (auth, cli_args) = extract_args(matches);
        // Note the Xpring API expects the asset code in lowercase
        let asset = cli_args["asset"].to_lowercase();
        let foreign_args: XpringResponse = self
            .client
            .get(&format!("https://xpring.io/api/accounts/{}", asset))
            .send()?
            .json()
            .map_err(Error::Testnet)?;
        let mut args = HashMap::new();
        let token = format!("{}:{}", foreign_args.username, foreign_args.passkey);
        args.insert("ilp_over_http_url", foreign_args.http_endpoint);
        args.insert("ilp_over_http_outgoing_token", token.clone());
        args.insert("ilp_over_btp_url", foreign_args.btp_endpoint);
        args.insert("ilp_over_btp_outgoing_token", token.clone());
        args.insert("asset_scale", foreign_args.asset_scale.to_string());
        args.insert("asset_code", foreign_args.asset_code);
        args.insert("username", format!("xpring_{}", asset));
        args.insert("routing_relation", String::from("Parent")); // TODO: weird behavior when deleting and re-inserting accounts with this
                                                                 // TODO should we set different parameters?
        args.insert("settle_threshold", "1000".to_string());
        args.insert("settle_to", "0".to_string());

        let result = self
            .client
            .post(&format!("{}/accounts/", self.url))
            .bearer_auth(auth)
            .json(&args)
            .send();

        if matches.is_present("return_testnet_credential") {
            result?;
            Ok(Response::from(
                http::Response::builder().body(token).unwrap(), // infallible unwrap
            ))
        } else {
            result.map_err(Error::Send)
        }
    }
}

// This function takes the map of arguments parsed by Clap
// and extracts the values for each argument.
fn extract_args<'a>(matches: &'a ArgMatches) -> (&'a str, HashMap<&'a str, &'a str>) {
    let mut args: HashMap<_, _> = matches // Contains data and metadata about the parsed command
        .args // The hashmap containing each parameter along with its values and metadata
        .iter()
        .map(|(&key, val)| (key, val.vals.get(0))) // Extract raw key/value pairs
        .filter(|(_, val)| val.is_some()) // Reject keys that don't have values
        .map(|(key, val)| (key, val.unwrap().to_str().unwrap())) // Convert values from bytes to strings
        .collect();
    let auth = args.remove("authorization_key").unwrap();
    (auth, args)
}

fn unflatten_pairs<'a>(matches: &'a ArgMatches) -> (&'a str, HashMap<&'a str, &'a str>) {
    let mut pairs = HashMap::new();
    if let Some(halve_matches) = matches.values_of("halve") {
        let halves: Vec<&str> = halve_matches.collect();
        for pair in halves.windows(2).step_by(2) {
            pairs.insert(pair[0], pair[1]);
        }
    }
    (matches.value_of("authorization_key").unwrap(), pairs)
}

#[derive(Debug, serde::Deserialize)]
struct XpringResponse {
    http_endpoint: String,
    passkey: String,
    btp_endpoint: String,
    asset_scale: u8,
    node: String,
    asset_code: String,
    username: String,
    payment_pointer: String,
}
