use clap::{value_t, App, Arg, SubCommand};
use hex;
use std::str::FromStr;
use tokio;

use futures::Future;
use interledger_settlement_engines::{
    engines::ethereum_ledger::{
        EthAddress, EthereumLedgerSettlementEngineBuilder, EthereumLedgerTxSigner,
    },
    stores::redis_ethereum_ledger::EthereumLedgerRedisStoreBuilder,
    SettlementEngineApi,
};
use interledger_store_redis::RedisStoreBuilder;
use log::info;
use redis::IntoConnectionInfo;
use ring::{digest, hmac};
use std::{net::SocketAddr, str, u64};
use tokio::net::TcpListener;
use url::Url;

static REDIS_SECRET_GENERATION_STRING: &str = "ilp_redis_secret";
pub fn generate_redis_secret(secret_seed: &[u8; 32]) -> [u8; 32] {
    let mut redis_secret: [u8; 32] = [0; 32];
    let sig = hmac::sign(
        &hmac::SigningKey::new(&digest::SHA256, secret_seed),
        REDIS_SECRET_GENERATION_STRING.as_bytes(),
    );
    redis_secret.copy_from_slice(sig.as_ref());
    redis_secret
}

#[allow(clippy::cognitive_complexity)]
pub fn main() {
    env_logger::init();

    let mut app = App::new("interledger-settlement-engines")
        .about("Interledger Settlement Engines CLI")
        .subcommands(vec![
            SubCommand::with_name("ethereum-ledger")
                .about("Ethereum settlement engine which performs ledger (layer 1) transactions")
                    .args(&[
                        Arg::with_name("port")
                            .long("port")
                            .help("Port to listen for settlement requests on")
                            .default_value("3000"),
                        Arg::with_name("key")
                            .long("key")
                            .help("private key for settlement account")
                            .takes_value(true)
                            .required(true),
                        Arg::with_name("ethereum_endpoint")
                            .long("ethereum_endpoint")
                            .help("Ethereum node endpoint")
                            .default_value("http://127.0.0.1:8545"),
                        Arg::with_name("token_address")
                            .long("token_address")
                            .help("The address of the ERC20 token to be used for settlement")
                            .default_value(""),
                        Arg::with_name("connector_url")
                            .long("connector_url")
                            .help("Connector Settlement API endpoint")
                            .default_value("http://127.0.0.1:7771"),
                        Arg::with_name("redis_uri")
                            .long("redis_uri")
                            .help("Redis database to add the account to")
                            .default_value("redis://127.0.0.1:6379"),
                        Arg::with_name("server_secret")
                            .long("server_secret")
                            .help("Cryptographic seed used to derive keys")
                            .takes_value(true)
                            .required(true),
                        Arg::with_name("chain_id")
                            .long("chain_id")
                            .help("The chain id so that the signer calculates the v value of the sig appropriately")
                            .default_value("1"),
                        Arg::with_name("confirmations")
                            .long("confirmations")
                            .help("The number of confirmations the engine will wait for a transaction's inclusion before it notifies the node of its success")
                            .default_value("6"),
                        Arg::with_name("poll_frequency")
                            .long("poll_frequency")
                            .help("The frequency in milliseconds at which the engine will check the blockchain about the confirmation status of a tx")
                            .default_value("5000"),
                        Arg::with_name("watch_incoming")
                            .long("watch_incoming")
                            .help("Launch a blockchain watcher that listens for incoming transactions and notifies the connector upon sufficient confirmations")
                            .default_value("true"),
                    ])
        ]
    );

    match app.clone().get_matches().subcommand() {
        ("ethereum-ledger", Some(matches)) => {
            let settlement_port =
                value_t!(matches, "port", u16).expect("port for settlement engine required");
            // TODO make compatible with
            // https://github.com/tendermint/signatory to have HSM sigs
            let private_key: String = value_t!(matches, "key", String).unwrap();
            let ethereum_endpoint: String =
                value_t!(matches, "ethereum_endpoint", String).unwrap();
            let token_address = value_t!(matches, "token_address", String).unwrap();
            let token_address = if token_address.len() == 20 {
                Some(EthAddress::from_str(&token_address).unwrap())
            } else {
                None
            };
            let connector_url: String = value_t!(matches, "connector_url", String).unwrap();
            let redis_uri =
                value_t!(matches, "redis_uri", String).expect("redis_uri is required");
            let redis_uri = Url::parse(&redis_uri).expect("redis_uri is not a valid URI");
            let server_secret: [u8; 32] = {
                let encoded: String = value_t!(matches, "server_secret", String).unwrap();
                let mut server_secret = [0; 32];
                let decoded = hex::decode(encoded).expect("server_secret must be hex-encoded");
                assert_eq!(decoded.len(), 32, "server_secret must be 32 bytes");
                server_secret.clone_from_slice(&decoded);
                server_secret
            };
            let chain_id = value_t!(matches, "chain_id", u8).unwrap();
            let confirmations = value_t!(matches, "confirmations", u8).unwrap();
            let poll_frequency = value_t!(matches, "poll_frequency", u64).unwrap();
            let watch_incoming = value_t!(matches, "watch_incoming", bool).unwrap();

            tokio::run(run_settlement_engine(
                redis_uri,
                ethereum_endpoint,
                settlement_port,
                &server_secret,
                private_key,
                chain_id,
                confirmations,
                poll_frequency,
                connector_url,
                token_address,
                watch_incoming,
            ));
        }
        _ => app.print_help().unwrap(),
    }
}

#[doc(hidden)]
#[allow(clippy::all)]
pub fn run_settlement_engine<R, Si>(
    redis_uri: R,
    ethereum_endpoint: String,
    settlement_port: u16,
    secret_seed: &[u8; 32],
    private_key: Si,
    chain_id: u8,
    confirmations: u8,
    poll_frequency: u64,
    connector_url: String,
    token_address: Option<EthAddress>,
    watch_incoming: bool,
) -> impl Future<Item = (), Error = ()>
where
    R: IntoConnectionInfo,
    Si: EthereumLedgerTxSigner + Clone + Send + Sync + 'static,
{
    let redis_secret = generate_redis_secret(secret_seed);
    let redis_uri = redis_uri.into_connection_info().unwrap();

    EthereumLedgerRedisStoreBuilder::new(redis_uri.clone())
        .connect()
        .and_then(move |ethereum_store| {
            let engine = EthereumLedgerSettlementEngineBuilder::new(ethereum_store, private_key)
                .ethereum_endpoint(&ethereum_endpoint)
                .chain_id(chain_id)
                .connector_url(&connector_url)
                .confirmations(confirmations)
                .poll_frequency(poll_frequency)
                .watch_incoming(watch_incoming)
                .token_address(token_address)
                .connect();

            RedisStoreBuilder::new(redis_uri, redis_secret)
                .connect()
                .and_then(move |store| {
                    let addr = SocketAddr::from(([127, 0, 0, 1], settlement_port));
                    let listener = TcpListener::bind(&addr)
                        .expect("Unable to bind to Settlement Engine address");
                    info!("Ethereum Settlement Engine listening on: {}", addr);

                    let api = SettlementEngineApi::new(engine, store);
                    tokio::spawn(api.serve(listener.incoming()));
                    Ok(())
                })
        })
}
