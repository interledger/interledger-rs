use super::types::{Addresses, EthereumAccount, EthereumLedgerTxSigner, EthereumStore};
use super::utils::{filter_transfer_logs, make_tx, sent_to_us, ERC20Transfer};
use clarity::Signature;
use log::{debug, error, trace};
use sha3::{Digest, Keccak256 as Sha3};
use std::collections::HashMap;
use std::iter::FromIterator;

use ethereum_tx_sign::web3::{
    api::Web3,
    futures::future::{err, join_all, ok, result, Either, Future},
    futures::stream::Stream,
    transports::Http,
    types::{Address, BlockNumber, CallRequest, TransactionId, H256, U256},
};
use hyper::StatusCode;
use interledger_store_redis::RedisStoreBuilder;
use log::info;
use num_bigint::BigUint;
use redis::IntoConnectionInfo;
use reqwest::r#async::{Client, Response as HttpResponse};
use ring::{digest, hmac};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::{
    marker::PhantomData,
    str::FromStr,
    time::{Duration, Instant},
};
use std::{net::SocketAddr, str, u64};
use tokio::net::TcpListener;
use tokio::timer::Interval;
use tokio_retry::{strategy::ExponentialBackoff, Retry};
use url::Url;
use uuid::Uuid;

use crate::stores::redis_ethereum_ledger::*;
use crate::{ApiResponse, CreateAccount, SettlementEngine, SettlementEngineApi};
use interledger_settlement::{Convert, ConvertDetails, Quantity};

const MAX_RETRIES: usize = 10;
const ETH_CREATE_ACCOUNT_PREFIX: &[u8] = b"ilp-ethl-create-account-message";

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PaymentDetailsResponse {
    to: Addresses,
    sig: Signature,
}

impl PaymentDetailsResponse {
    fn new(to: Addresses, sig: Signature) -> Self {
        PaymentDetailsResponse { to, sig }
    }
}

/// # Ethereum Ledger Settlement Engine
///
/// Settlement Engine compliant to [RFC536](https://github.com/interledger/rfcs/pull/536/)
///
/// The engine connects to an Ethereum node (over HTTP) as well as the connector. Its
/// functions are exposed via the Settlement Engine API.
///
/// It requires a `confirmations` security parameter which is used to ensure
/// that all transactions that get sent to the connector have sufficient
/// confirmations (suggested value: >6)
///
/// All settlements made with this engine make on-chain Layer 1 Ethereum
/// transactions. This engine DOES NOT support payment channels.
#[derive(Debug, Clone)]
pub struct EthereumLedgerSettlementEngine<S, Si, A> {
    store: S,
    signer: Si,
    account_type: PhantomData<A>,

    // Configuration data
    web3: Web3<Http>,
    address: Addresses,
    chain_id: u8,
    confirmations: u8,
    poll_frequency: Duration,
    connector_url: Url,
    asset_scale: u8,
}

pub struct EthereumLedgerSettlementEngineBuilder<'a, S, Si, A> {
    store: S,
    signer: Si,

    /// Ethereum Endpoint, default localhost:8545
    ethereum_endpoint: Option<&'a str>,
    chain_id: Option<u8>,
    confirmations: Option<u8>,
    poll_frequency: Option<Duration>,
    connector_url: Option<Url>,
    token_address: Option<Address>,
    asset_scale: Option<u8>,
    watch_incoming: bool,
    account_type: PhantomData<A>,
}

impl<'a, S, Si, A> EthereumLedgerSettlementEngineBuilder<'a, S, Si, A>
where
    S: EthereumStore<Account = A> + Clone + Send + Sync + 'static,
    Si: EthereumLedgerTxSigner + Clone + Send + Sync + 'static,
    A: EthereumAccount + Send + Sync + 'static,
{
    pub fn new(store: S, signer: Si) -> Self {
        Self {
            store,
            signer,
            ethereum_endpoint: None,
            chain_id: None,
            confirmations: None,
            poll_frequency: None,
            connector_url: None,
            token_address: None,
            asset_scale: None,
            watch_incoming: false,
            account_type: PhantomData,
        }
    }

    pub fn token_address(&mut self, token_address: Option<Address>) -> &mut Self {
        self.token_address = token_address;
        self
    }

    pub fn ethereum_endpoint(&mut self, endpoint: &'a str) -> &mut Self {
        self.ethereum_endpoint = Some(endpoint);
        self
    }

    pub fn asset_scale(&mut self, asset_scale: u8) -> &mut Self {
        self.asset_scale = Some(asset_scale);
        self
    }

    pub fn chain_id(&mut self, chain_id: u8) -> &mut Self {
        self.chain_id = Some(chain_id);
        self
    }

    pub fn confirmations(&mut self, confirmations: u8) -> &mut Self {
        self.confirmations = Some(confirmations);
        self
    }

    /// The frequency to check for new blocks for in milliseconds
    pub fn poll_frequency(&mut self, poll_frequency: u64) -> &mut Self {
        self.poll_frequency = Some(Duration::from_millis(poll_frequency));
        self
    }

    pub fn watch_incoming(&mut self, watch_incoming: bool) -> &mut Self {
        self.watch_incoming = watch_incoming;
        self
    }

    pub fn connector_url(&mut self, connector_url: &'a str) -> &mut Self {
        self.connector_url = Some(connector_url.parse().unwrap());
        self
    }

    pub fn connect(&self) -> EthereumLedgerSettlementEngine<S, Si, A> {
        let ethereum_endpoint = if let Some(ref ethereum_endpoint) = self.ethereum_endpoint {
            &ethereum_endpoint
        } else {
            "http://localhost:8545"
        };
        let chain_id = if let Some(chain_id) = self.chain_id {
            chain_id
        } else {
            1
        };
        let connector_url = if let Some(connector_url) = self.connector_url.clone() {
            connector_url
        } else {
            "http://localhost:7771".parse().unwrap()
        };
        let confirmations = if let Some(confirmations) = self.confirmations {
            confirmations
        } else {
            6
        };
        let poll_frequency = if let Some(poll_frequency) = self.poll_frequency {
            poll_frequency
        } else {
            Duration::from_secs(5)
        };
        let asset_scale = if let Some(asset_scale) = self.asset_scale {
            asset_scale
        } else {
            18
        };

        let (eloop, transport) = Http::new(ethereum_endpoint).unwrap();
        eloop.into_remote();
        let web3 = Web3::new(transport);
        let address = Addresses {
            own_address: self.signer.address(),
            token_address: self.token_address,
        };

        let engine = EthereumLedgerSettlementEngine {
            web3,
            store: self.store.clone(),
            signer: self.signer.clone(),
            address,
            chain_id,
            confirmations,
            poll_frequency,
            connector_url,
            asset_scale,
            account_type: PhantomData,
        };
        if self.watch_incoming {
            engine.notify_connector_on_incoming_settlement();
        }
        engine
    }
}

impl<S, Si, A> EthereumLedgerSettlementEngine<S, Si, A>
where
    S: EthereumStore<Account = A> + Clone + Send + Sync + 'static,
    Si: EthereumLedgerTxSigner + Clone + Send + Sync + 'static,
    A: EthereumAccount + Send + Sync + 'static,
{
    /// Periodically spawns a job every `self.poll_frequency` that notifies the
    /// Settlement Engine's connectors about transactions which are sent to the
    /// engine's address.
    pub fn notify_connector_on_incoming_settlement(&self) {
        let _self = self.clone();
        let interval = self.poll_frequency;
        let address = self.address;
        debug!(
            "[{:?}] settlement engine service for listening to incoming settlements. Interval: {:?}",
            address, interval,
        );
        std::thread::spawn(move || {
            tokio::run(
                Interval::new(Instant::now(), interval)
                    .for_each(move |instant| {
                        trace!(
                            "[{:?}] Getting settlement data from the blockchain; instant={:?}",
                            address,
                            instant
                        );
                        tokio::spawn(_self.handle_received_transactions());
                        Ok(())
                    })
                    .map_err(|e| panic!("interval errored; err={:?}", e)),
            );
        });
    }

    /// Routine for notifying the connector about incoming transactions.
    /// Algorithm (heavily unoptimized):
    /// 1. Fetch the last observed block number
    /// 2. Fetch the current block number from Ethereum
    /// 3. Fetch all blocks since the last observed one,
    ///    until $(current block number - confirmations), where $confirmations
    ///    is a security parameter to be safe against block reorgs.
    /// 4. For each block (in parallel):
    ///     For each transaction (in parallel):
    ///     1. Skip if it is not sent to our address or have 0 value.
    ///     2. Fetch the id that matches its sender from the store
    ///     3. Notify the connector about it by making a POST request to the connector's
    ///        /accounts/$id/settlements endpoint with transaction's value as the
    ///        body. This call is retried if it fails.
    /// 5. Save (current block number - confirmations) to be used as
    ///    last observed data for the next call of this function.
    // Optimization: This will make 1 settlement API request to
    // the connector per transaction that comes to us. Since
    // we're scanning across multiple blocks, we want to
    // optimize it so that: it gathers a mapping of
    // (accountId=>amount) across all transactions and blocks, and
    // only makes 1 transaction per accountId with the
    // appropriate amount.
    pub fn handle_received_transactions(&self) -> impl Future<Item = (), Error = ()> + Send {
        let confirmations = self.confirmations;
        let web3 = self.web3.clone();
        let store = self.store.clone();
        let store_clone = self.store.clone();
        let self_clone = self.clone();
        let self_clone2 = self.clone();
        let our_address = self.address.own_address;
        let token_address = self.address.token_address;

        // get the current block number
        web3.eth()
            .block_number()
            .map_err(move |err| error!("Could not fetch current block number {:?}", err))
            .and_then(move |current_block| {
                trace!("Current block {}", current_block);
                // get the safe number of blocks to avoid reorgs
                let fetch_until = current_block - confirmations;
                // U256 does not implement IntoFuture so we must wrap it
                Ok((Ok(fetch_until), store.load_recently_observed_block()))
            })
            .flatten()
            .and_then(move |(fetch_until, last_observed_block)| {
                trace!(
                    "Will fetch txs from block {} until {}",
                    last_observed_block,
                    fetch_until
                );

                let notify_all_txs_fut = if let Some(token_address) = token_address {
                    trace!("Settling for ERC20 transactions");
                    // get all erc20 transactions
                    let notify_all_erc20_txs_fut = filter_transfer_logs(
                        web3.clone(),
                        token_address,
                        None,
                        Some(our_address),
                        BlockNumber::Number(last_observed_block.low_u64()),
                        BlockNumber::Number(fetch_until.low_u64()),
                    )
                    .and_then(move |transfers: Vec<ERC20Transfer>| {
                        // map each incoming erc20 transactions to an outgoing
                        // notification to the connector
                        join_all(transfers.into_iter().map(move |transfer| {
                            self_clone2.notify_erc20_transfer(transfer, token_address)
                        }))
                    });

                    // combine all erc20 futures for that range of blocks
                    Either::A(notify_all_erc20_txs_fut)
                } else {
                    trace!("Settling for ETH transactions");
                    let checked_blocks = last_observed_block.low_u64()..=fetch_until.low_u64();
                    // for each block create a future which will notify the
                    // connector about all the transactions in that block that are sent to our account
                    let notify_eth_txs_fut = checked_blocks
                        .map(move |block_num| self_clone.notify_eth_txs_in_block(block_num));

                    // combine all the futures for that range of blocks
                    Either::B(join_all(notify_eth_txs_fut))
                };

                notify_all_txs_fut.and_then(move |ret| {
                    trace!("Transactions settled {:?}", ret);
                    // now that all transactions have been processed successfully, we
                    // can save `fetch_until` as the latest observed block
                    store_clone.save_recently_observed_block(fetch_until)
                })
            })
    }

    /// Submits an ERC20 transfer object's data to the connector
    // todo: Try combining the body of this function with `notify_eth_transfer`
    fn notify_erc20_transfer(
        &self,
        transfer: ERC20Transfer,
        token_address: Address,
    ) -> impl Future<Item = (), Error = ()> {
        let store = self.store.clone();
        let tx_hash = transfer.tx_hash;
        let self_clone = self.clone();
        let addr = Addresses {
            own_address: transfer.from,
            token_address: Some(token_address),
        };
        let amount = transfer.amount;
        store
            .check_if_tx_processed(tx_hash)
            .map_err(move |_| error!("Error when querying store about transaction: {:?}", tx_hash))
            .and_then(move |processed| {
                if !processed {
                    Either::A(
                        store
                            .load_account_id_from_address(addr)
                            .and_then(move |id| {
                                self_clone.notify_connector(id.to_string(), amount, tx_hash)
                            })
                            .and_then(move |_| {
                                // only save the transaction hash if the connector
                                // was successfully notified
                                store.mark_tx_processed(tx_hash)
                            }),
                    )
                } else {
                    Either::B(ok(())) // return an empty future otherwise since we want to skip this transaction
                }
            })
    }

    fn notify_eth_txs_in_block(&self, block_number: u64) -> impl Future<Item = (), Error = ()> {
        trace!("Getting txs for block {}", block_number);
        let self_clone = self.clone();
        // Get the block at `block_number`
        self.web3
            .eth()
            .block(BlockNumber::Number(block_number).into())
            .map_err(move |err| error!("Got error while getting block {}: {:?}", block_number, err))
            .and_then(move |maybe_block| {
                // Error out if the block was not found (unlikely to occur since we're only
                // calling this for past blocks)
                if let Some(block) = maybe_block {
                    ok(block)
                } else {
                    err(())
                }
            })
            .and_then(move |block| {
                // for each transaction inside the block, submit it to the
                // connector if it was sent to us
                let submit_txs_to_connector_future = block
                    .transactions
                    .into_iter()
                    .map(move |tx_hash| self_clone.notify_eth_transfer(tx_hash));

                // combine all the futures for that block's transactions
                join_all(submit_txs_to_connector_future)
            })
            .and_then(move |_res| {
                trace!(
                    "Successfully logged transactions in block: {:?}",
                    block_number
                );
                ok(())
            })
    }

    fn notify_eth_transfer(&self, tx_hash: H256) -> impl Future<Item = (), Error = ()> {
        let our_address = self.address.own_address;
        let web3 = self.web3.clone();
        let store = self.store.clone();
        let self_clone = self.clone();
        // Skip transactions which have already been processed by the connector
        store.check_if_tx_processed(tx_hash)
        .map_err(move |_| error!("Error when querying store about transaction: {:?}", tx_hash))
        .and_then(move |processed| {
            if !processed {
                Either::A(
                web3.eth().transaction(TransactionId::Hash(tx_hash))
                .map_err(move |err| error!("Could not fetch transaction data from transaction hash: {:?}. Got error: {:?}", tx_hash, err))
                .and_then(move |maybe_tx| {
                    // Unlikely to error out since we only call this on
                    // transaction hashes which were inside a mined block that
                    // had many confirmations
                    if let Some(tx) = maybe_tx { ok(tx) } else { err(())}
                })
                .and_then(move |tx| {
                    trace!("Inspecting transaction: {:?}", tx);
                    let (from, amount, token_address) = sent_to_us(tx, our_address);
                    if amount > U256::from(0) {
                        // if the tx was for us and had some non-0 amount, then let
                        // the connector know
                        let addr = Addresses {
                            own_address: from,
                            token_address,
                        };
                        Either::A(
                        store.load_account_id_from_address(addr)
                        .and_then(move |id| {
                            self_clone.notify_connector(id.to_string(), amount, tx_hash)
                        })
                        .and_then(move |_| {
                            // only save the transaction hash if the connector
                            // was successfully notified
                            store.mark_tx_processed(tx_hash)
                        }))
                    } else {
                        // otherwise return an empty future
                        Either::B(ok(()))
                    }
                }))
            } else {
                Either::B(ok(())) // return an empty future otherwise since we want to skip this transaction
            }
        })
    }

    fn notify_connector(
        &self,
        account_id: String,
        amount: U256,
        tx_hash: H256,
    ) -> impl Future<Item = (), Error = ()> {
        let mut url = self.connector_url.clone();
        let account_id_clone = account_id.clone();
        let engine_scale = self.asset_scale;
        url.path_segments_mut()
            .expect("Invalid connector URL")
            .push("accounts")
            .push(&account_id.clone())
            .push("settlements");
        let client = Client::new();
        debug!("Making POST to {:?} {:?} about {:?}", url, amount, tx_hash);
        let action = move || {
            let account_id = account_id.clone();
            client
                .post(url.clone())
                .header("Idempotency-Key", tx_hash.to_string())
                .json(&json!({ "amount": amount.to_string(), "scale" : engine_scale }))
                .send()
                .map_err(move |err| {
                    error!(
                        "Error notifying Accounting System's account: {:?}, amount: {:?}: {:?}",
                        account_id, amount, err
                    )
                })
                .and_then(move |response| {
                    trace!("Accounting system responded with {:?}", response);
                    if response.status().is_success() {
                        Ok(())
                    } else {
                        Err(())
                    }
                })
        };
        Retry::spawn(
            ExponentialBackoff::from_millis(10).take(MAX_RETRIES),
            action,
        )
        .map_err(move |_| {
            error!("Exceeded max retries when notifying connector about account {:?} for amount {:?} and transaction hash {:?}. Please check your API.", account_id_clone, amount, tx_hash)
        })
    }

    /// Helper function which submits an Ethereum ledger transaction to `to` for `amount`.
    /// If called with `token_address`, it makes an ERC20 transaction instead.
    /// Due to the lack of an API to create and sign the transaction
    /// automatically it has to be done manually as follows:
    /// 1. fetch the account's nonce
    /// 2. construct the raw transaction using the nonce and the provided parameters
    /// 3. Sign the transaction (along with the chain id, due to EIP-155)
    /// 4. Submit the RLP-encoded transaction to the network
    fn settle_to(
        &self,
        to: Address,
        amount: U256,
        token_address: Option<Address>,
    ) -> Box<dyn Future<Item = H256, Error = ()> + Send> {
        let web3 = self.web3.clone();
        let own_address = self.address.own_address;
        let chain_id = self.chain_id;
        let signer = self.signer.clone();

        let mut tx = make_tx(to, amount, token_address);
        let gas_amount_fut = web3.eth().estimate_gas(
            CallRequest {
                to,
                from: None,
                gas: None,
                gas_price: None,
                value: Some(tx.value),
                data: Some(tx.data.clone().into()),
            },
            None,
        );
        let gas_price_fut = web3.eth().gas_price();
        let nonce_fut = web3
            .eth()
            .transaction_count(own_address, Some(BlockNumber::Pending));
        Box::new(
            join_all(vec![gas_price_fut, gas_amount_fut, nonce_fut])
                .map_err(|err| error!("Error when querying gas price / nonce: {:?}", err))
                .and_then(move |data| {
                    tx.gas_price = data[0];
                    tx.gas = data[1];
                    tx.nonce = data[2];

                    let signed_tx = signer.sign_raw_tx(tx.clone(), chain_id); // 3
                    let action = move || {
                        web3.eth() // 4
                            .send_raw_transaction(signed_tx.clone().into())
                    };
                    Retry::spawn(
                        ExponentialBackoff::from_millis(10).take(MAX_RETRIES),
                        action,
                    )
                    .map_err(move |err| {
                        error!("Error when sending transaction {:?}: {:?}", tx, err)
                    })
                    .and_then(move |tx_hash| {
                        debug!("Transaction submitted. Hash: {:?}", tx_hash);
                        Ok(tx_hash)
                    })
                }),
        )
    }

    /// Helper function that returns the addresses associated with an
    /// account from a given string account id
    fn load_account(
        &self,
        account_id: String,
    ) -> impl Future<Item = (A::AccountId, Addresses), Error = String> {
        let store = self.store.clone();
        let addr = self.address;
        result(A::AccountId::from_str(&account_id).map_err(move |_err| {
            let error_msg = "Unable to parse account".to_string();
            error!("{}", error_msg);
            error_msg
        }))
        .and_then(move |account_id| {
            store
                .load_account_addresses(vec![account_id])
                .map_err(move |_err| {
                    let error_msg = format!("[{:?}] Error getting account: {}", addr, account_id);
                    error!("{}", error_msg);
                    error_msg
                })
                .and_then(move |addresses| ok((account_id, addresses[0])))
        })
    }
}

impl<S, Si, A> SettlementEngine for EthereumLedgerSettlementEngine<S, Si, A>
where
    S: EthereumStore<Account = A> + Clone + Send + Sync + 'static,
    Si: EthereumLedgerTxSigner + Clone + Send + Sync + 'static,
    A: EthereumAccount + Send + Sync + 'static,
{
    /// Settlement Engine's function that corresponds to the
    /// /accounts/:id/ endpoint (POST). It queries the connector's
    /// accounts/:id/messages with a "PaymentDetailsRequest" that gets forwarded
    /// to the connector's peer that matches the provided account id, which
    /// finally gets forwarded to the peer SE's receive_message endpoint which
    /// responds with its Ethereum and Token addresses. Upon
    /// receival of Ethereum and Token addresses from the peer, it saves them in
    /// the store.
    fn create_account(
        &self,
        account_id: CreateAccount,
    ) -> Box<dyn Future<Item = ApiResponse, Error = ApiResponse> + Send> {
        let self_clone = self.clone();
        let store: S = self.store.clone();
        let account_id = account_id.id;

        Box::new(
            result(A::AccountId::from_str(&account_id).map_err({
                move |_err| {
                    let error_msg = "Unable to parse account".to_string();
                    error!("{}", error_msg);
                    let status_code = StatusCode::from_u16(400).unwrap();
                    (status_code, error_msg)
                }
            }))
            .and_then(move |account_id| {
                // We make a POST request to OUR connector's `messages`
                // endpoint. This will in turn send an outgoing
                // request to its peer connector, which will ask its
                // own engine about its settlement information. Then,
                // we store that information and use it when
                // performing settlements.
                let idempotency_uuid = Uuid::new_v4().to_hyphenated().to_string();
                let challenge = Uuid::new_v4().to_hyphenated().to_string();
                let challenge = challenge.into_bytes();
                let challenge_clone = challenge.clone();
                let client = Client::new();
                let mut url = self_clone.connector_url.clone();
                url.path_segments_mut()
                    .expect("Invalid connector URL")
                    .push("accounts")
                    .push(&account_id.to_string())
                    .push("messages");
                let action = move || {
                    client
                        .post(url.clone())
                        .header("Content-Type", "application/octet-stream")
                        .header("Idempotency-Key", idempotency_uuid.clone())
                        .body(challenge.clone())
                        .send()
                };

                Retry::spawn(
                    ExponentialBackoff::from_millis(10).take(MAX_RETRIES),
                    action,
                )
                .map_err(move |err| {
                    let err = format!("Couldn't notify connector {:?}", err);
                    error!("{}", err);
                    (StatusCode::from_u16(500).unwrap(), err)
                })
                .and_then(move |resp| {
                    parse_body_into_payment_details(resp).and_then(move |payment_details| {
                        let data = prefixed_mesage(challenge_clone);
                        let challenge_hash = Sha3::digest(&data);
                        let recovered_address = payment_details.sig.recover(&challenge_hash);
                        trace!("Received payment details {:?}", payment_details);
                        result(recovered_address)
                            .map_err(move |err| {
                                let err = format!("Could not recover address {:?}", err);
                                error!("{}", err);
                                (StatusCode::from_u16(502).unwrap(), err)
                            })
                            .and_then({
                                let payment_details = payment_details.clone();
                                move |recovered_address| {
                                    if recovered_address.as_bytes()
                                        == &payment_details.to.own_address.to_vec()[..]
                                    {
                                        ok(())
                                    } else {
                                        let error_msg = format!(
                                            "Recovered address did not match: {:?}. Expected {:?}",
                                            recovered_address.to_string(),
                                            payment_details.to
                                        );
                                        error!("{}", error_msg);
                                        err((StatusCode::from_u16(502).unwrap(), error_msg))
                                    }
                                }
                            })
                            .and_then(move |_| {
                                let data =
                                    HashMap::from_iter(vec![(account_id, payment_details.to)]);
                                store.save_account_addresses(data).map_err(move |err| {
                                    let err = format!("Couldn't connect to store {:?}", err);
                                    error!("{}", err);
                                    (StatusCode::from_u16(500).unwrap(), err)
                                })
                            })
                    })
                })
                .and_then(move |_| Ok((StatusCode::from_u16(201).unwrap(), "CREATED".to_owned())))
            }),
        )
    }

    /// Settlement Engine's function that corresponds to the
    /// /accounts/:id/messages endpoint (POST).
    /// The body is a challenge issued by the peer's settlement engine which we
    /// must sign to prove ownership of our address
    fn receive_message(
        &self,
        account_id: String,
        body: Vec<u8>,
    ) -> Box<dyn Future<Item = ApiResponse, Error = ApiResponse> + Send> {
        let address = self.address;
        // We are only returning our information, so
        // there is no need to return any data about the
        // provided account.
        debug!(
            "Responding with our account's details {} {:?}",
            account_id, address
        );
        let data = prefixed_mesage(body.clone());
        let signature = self.signer.sign_message(&data);
        let resp = {
            let ret = PaymentDetailsResponse::new(address, signature);
            serde_json::to_string(&ret).unwrap()
        };
        Box::new(ok((StatusCode::from_u16(200).unwrap(), resp)))
    }
    /// Settlement Engine's function that corresponds to the
    /// /accounts/:id/settlements endpoint (POST). It performs an Ethereum
    /// onchain transaction to the Ethereum Address that corresponds to the
    /// provided account id, for the amount specified in the message's body. If
    /// the account is associated with an ERC20 token, it makes an ERC20 call instead.
    fn send_money(
        &self,
        account_id: String,
        body: Quantity,
    ) -> Box<dyn Future<Item = ApiResponse, Error = ApiResponse> + Send> {
        let self_clone = self.clone();
        let engine_scale = self.asset_scale;
        Box::new(
            result(BigUint::from_str(&body.amount).map_err(move |err| {
                let error_msg = format!("Error converting to U256 {:?}", err);
                error!("{:?}", error_msg);
                (StatusCode::from_u16(400).unwrap(), error_msg)
            }))
            .and_then(move |amount| {
                // If we receive a Quantity { amount: "1", scale: 9},
                // we must normalize it to our engine's scale (typically 18
                // for ETH/ERC20).
                // Slightly counter-intuitive that `body.scale` is set as `to`.
                let amount = amount.normalize_scale(ConvertDetails {
                    from: engine_scale,
                    to: body.scale,
                });

                // Typecast from num_bigint::BigUInt because we're using
                // ethereum_types::U256 for all rust-web3 related functionality
                result(U256::from_dec_str(&amount.to_string()).map_err(move |err| {
                    let error_msg = format!("Error converting to U256 {:?}", err);
                    error!("{:?}", error_msg);
                    (StatusCode::from_u16(400).unwrap(), error_msg)
                }))
            })
            .and_then(move |amount| {
                self_clone
                    .load_account(account_id)
                    .map_err(move |err| {
                        let error_msg = format!("Error loading account {:?}", err);
                        error!("{}", error_msg);
                        (StatusCode::from_u16(400).unwrap(), error_msg)
                    })
                    .and_then(move |(_account_id, addresses)| {
                        self_clone
                            .settle_to(addresses.own_address, amount, addresses.token_address)
                            .map_err(move |_| {
                                let error_msg = "Error connecting to the blockchain.".to_string();
                                error!("{}", error_msg);
                                (StatusCode::from_u16(502).unwrap(), error_msg)
                            })
                    })
                    .and_then(move |_| Ok((StatusCode::OK, "OK".to_string())))
            }),
        )
    }
}

fn parse_body_into_payment_details(
    resp: HttpResponse,
) -> impl Future<Item = PaymentDetailsResponse, Error = ApiResponse> {
    resp.into_body()
        .concat2()
        .map_err(|err| {
            let err = format!("Couldn't retrieve body {:?}", err);
            error!("{}", err);
            (StatusCode::from_u16(500).unwrap(), err)
        })
        .and_then(move |body| {
            serde_json::from_slice::<PaymentDetailsResponse>(&body).map_err(|err| {
                let err = format!(
                    "Couldn't parse body {:?} into payment details {:?}",
                    body, err
                );
                error!("{}", err);
                (StatusCode::from_u16(500).unwrap(), err)
            })
        })
}

fn prefixed_mesage(challenge: Vec<u8>) -> Vec<u8> {
    let mut ret = ETH_CREATE_ACCOUNT_PREFIX.to_vec();
    ret.extend(challenge);
    ret
}

#[doc(hidden)]
#[allow(clippy::all)]
pub fn run_ethereum_engine<R, Si>(
    redis_uri: R,
    ethereum_endpoint: String,
    settlement_port: u16,
    secret_seed: &[u8; 32],
    private_key: Si,
    chain_id: u8,
    confirmations: u8,
    asset_scale: u8,
    poll_frequency: u64,
    connector_url: String,
    token_address: Option<Address>,
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
                .asset_scale(asset_scale)
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

static REDIS_SECRET_GENERATION_STRING: &str = "ilp_redis_secret";
fn generate_redis_secret(secret_seed: &[u8; 32]) -> [u8; 32] {
    let mut redis_secret: [u8; 32] = [0; 32];
    let sig = hmac::sign(
        &hmac::SigningKey::new(&digest::SHA256, secret_seed),
        REDIS_SECRET_GENERATION_STRING.as_bytes(),
    );
    redis_secret.copy_from_slice(sig.as_ref());
    redis_secret
}

#[cfg(test)]
mod tests {
    use super::super::fixtures::{ALICE, BOB, MESSAGES_API};
    use super::super::test_helpers::{
        block_on, start_ganache, test_engine, test_store, TestAccount,
    };
    use super::*;
    use lazy_static::lazy_static;
    use mockito;

    lazy_static! {
        pub static ref ALICE_PK: String =
            String::from("380eb0f3d505f087e438eca80bc4df9a7faa24f868e69fc0440261a0fc0567dc");
        pub static ref BOB_PK: String =
            String::from("cc96601bc52293b53c4736a12af9130abf347669b3813f9ec4cafdf6991b087e");
    }

    #[test]
    // All tests involving ganache must be run in 1 suite so that they run serially
    fn test_send_money() {
        let _ = env_logger::try_init();
        let alice = ALICE.clone();
        let bob = BOB.clone();

        let alice_store = test_store(ALICE.clone(), false, false, true);
        alice_store
            .save_account_addresses(HashMap::from_iter(vec![(
                0,
                Addresses {
                    own_address: bob.address,
                    token_address: None,
                },
            )]))
            .wait()
            .unwrap();

        let bob_store = test_store(bob.clone(), false, false, true);
        bob_store
            .save_account_addresses(HashMap::from_iter(vec![(
                42,
                Addresses {
                    own_address: alice.address,
                    token_address: None,
                },
            )]))
            .wait()
            .unwrap();

        let mut ganache_pid = start_ganache();

        let bob_mock = mockito::mock("POST", "/accounts/42/settlements")
            .match_body(mockito::Matcher::JsonString(
                "{\"amount\": \"100000000000\", \"scale\": 18 }".to_string(),
            ))
            .with_status(200)
            .with_body("OK".to_string())
            .create();

        let bob_connector_url = mockito::server_url();
        let _bob_engine = test_engine(
            bob_store.clone(),
            BOB_PK.clone(),
            0,
            &bob_connector_url,
            true,
        );

        let alice_engine = test_engine(
            alice_store.clone(),
            ALICE_PK.clone(),
            0,
            "http://127.0.0.1:9999",
            false, // alice sends the transaction to bob (set it up so that she doesn't listen for inc txs)
        );

        let ret =
            block_on(alice_engine.send_money(bob.id.to_string(), Quantity::new(100, 9))).unwrap();
        assert_eq!(ret.0.as_u16(), 200);
        assert_eq!(ret.1, "OK");

        std::thread::sleep(Duration::from_millis(2000)); // wait a few seconds so that the receiver's engine that does the polling

        ganache_pid.kill().unwrap(); // kill ganache since it's no longer needed
        bob_mock.assert();
    }

    #[test]
    fn test_create_get_account() {
        let bob: TestAccount = BOB.clone();

        let challenge = Uuid::new_v4().to_hyphenated().to_string();
        let signature = BOB_PK.clone().sign_message(&challenge.clone().into_bytes());

        let body_se_data = serde_json::to_string(&PaymentDetailsResponse {
            to: Addresses {
                own_address: bob.address,
                token_address: None,
            },
            sig: signature,
        })
        .unwrap();

        // simulate our connector that accepts the request, forwards it to the
        // peer's connector and returns the peer's se addresses
        let m = mockito::mock("POST", MESSAGES_API.clone())
            .with_status(200)
            .with_body(body_se_data)
            .expect(1) // only 1 request is made to the connector (idempotency works properly)
            .create();
        let connector_url = mockito::server_url();

        // alice sends a create account request to her engine
        // which makes a call to bob's connector which replies with bob's
        // address and signature on the challenge
        let store = test_store(bob.clone(), false, false, false);
        let engine = test_engine(store.clone(), ALICE_PK.clone(), 0, &connector_url, false);

        // the signed message does not match. We are not able to make Mockito
        // capture the challenge and return a signature on it.
        let ret = block_on(engine.create_account(CreateAccount::new(bob.id))).unwrap_err();
        assert_eq!(ret.0.as_u16(), 502);
        // assert_eq!(ret.1, "CREATED");

        m.assert();
    }

    #[test]
    fn test_receive_message() {
        let bob: TestAccount = BOB.clone();

        let challenge = Uuid::new_v4().to_hyphenated().to_string().into_bytes();
        let signed_challenge = prefixed_mesage(challenge.clone());

        let signature = ALICE_PK.clone().sign_message(&signed_challenge);

        let store = test_store(ALICE.clone(), false, false, false);
        let engine = test_engine(
            store.clone(),
            ALICE_PK.clone(),
            0,
            "http://127.0.0.1:8770",
            false,
        );

        // Alice's engine receives a challenge by Bob.
        let ret = block_on(engine.receive_message(bob.id.to_string(), challenge)).unwrap();
        assert_eq!(ret.0.as_u16(), 200);

        let alice_addrs = Addresses {
            own_address: ALICE.address,
            token_address: None,
        };
        let data: PaymentDetailsResponse = serde_json::from_str(&ret.1).unwrap();
        // The returned addresses must be Alice's
        assert_eq!(data.to, alice_addrs);
        // The returned signature must be Alice's sig.
        assert_eq!(data.sig, signature);
    }
}
