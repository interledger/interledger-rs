use super::types::{Addresses, EthereumAccount, EthereumLedgerTxSigner, EthereumStore};
use super::utils::{filter_transfer_logs, make_tx, sent_to_us, ERC20Transfer};
use clarity::Signature;
use log::{debug, error, trace};
use sha3::{Digest, Keccak256 as Sha3};
use std::collections::HashMap;
use std::iter::FromIterator;

use hyper::StatusCode;
use log::info;
use num_bigint::BigUint;
use redis::IntoConnectionInfo;
use reqwest::r#async::{Client, Response as HttpResponse};
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
use web3::{
    api::Web3,
    futures::future::{err, join_all, ok, result, Either, Future},
    futures::stream::Stream,
    transports::Http,
    types::{Address, BlockNumber, CallRequest, TransactionId, H256, U256},
};

use crate::stores::{redis_ethereum_ledger::*, LeftoversStore};
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
    S: EthereumStore<Account = A>
        + LeftoversStore<AssetType = BigUint>
        + Clone
        + Send
        + Sync
        + 'static,
    Si: EthereumLedgerTxSigner + Clone + Send + Sync + 'static,
    A: EthereumAccount<AccountId = String> + Clone + Send + Sync + 'static,
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
    S: EthereumStore<Account = A>
        + LeftoversStore<AssetType = BigUint>
        + Clone
        + Send
        + Sync
        + 'static,
    Si: EthereumLedgerTxSigner + Clone + Send + Sync + 'static,
    A: EthereumAccount<AccountId = String> + Clone + Send + Sync + 'static,
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
                    .map_err(|e| panic!("interval errored; err={:?}", e))
                    .for_each(move |_| _self.handle_received_transactions())
                    .then(|_| {
                        // Don't stop loop even if there was an error
                        Ok(())
                    }),
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

        // We `Box` futures in these functions due to
        // https://github.com/rust-lang/rust/issues/54540#issuecomment-494749912.
        // Otherwise, we get `type_length_limit` errors.
        // get the current block number
        web3.eth()
            .block_number()
            .map_err(move |err| error!("Could not fetch current block number {:?}", err))
            .and_then(move |current_block| {
                // get the safe number of blocks to avoid reorgs
                let fetch_until = current_block - confirmations;
                // U256 does not implement IntoFuture so we must wrap it
                Ok((Ok(fetch_until), store.load_recently_observed_block()))
            })
            .flatten()
            .and_then(move |(to_block, last_observed_block)| {
                // If we are just starting up, fetch only the most recent block
                // Note this means we will ignore transactions that were received before
                // the first time the settlement engine was started.
                let from_block = if let Some(last_observed_block) = last_observed_block {
                    if to_block == last_observed_block {
                        // We already processed the latest block
                        return Either::A(ok(()));
                    } else {
                        last_observed_block + 1
                    }
                } else {
                    // Check only the latest block
                    to_block
                };

                trace!("Fetching txs from block {} until {}", from_block, to_block);

                let notify_all_txs_fut = if let Some(token_address) = token_address {
                    // get all erc20 transactions
                    let notify_all_erc20_txs_fut = filter_transfer_logs(
                        web3.clone(),
                        token_address,
                        None,
                        Some(our_address),
                        BlockNumber::Number(from_block.low_u64()),
                        BlockNumber::Number(to_block.low_u64()),
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
                    let checked_blocks = from_block.low_u64()..=to_block.low_u64();
                    // for each block create a future which will notify the
                    // connector about all the transactions in that block that are sent to our account
                    let notify_eth_txs_fut = checked_blocks
                        .map(move |block_num| self_clone.notify_eth_txs_in_block(block_num));

                    // combine all the futures for that range of blocks
                    Either::B(join_all(notify_eth_txs_fut))
                };

                Either::B(notify_all_txs_fut.and_then(move |_| {
                    trace!("Processed all transctions up to block {}", to_block);
                    // now that all transactions have been processed successfully, we
                    // can save `to_block` as the latest observed block
                    store_clone.save_recently_observed_block(to_block)
                }))
            })
    }

    /// Submits an ERC20 transfer object's data to the connector
    // todo: Try combining the body of this function with `notify_eth_transfer`
    fn notify_erc20_transfer(
        &self,
        transfer: ERC20Transfer,
        token_address: Address,
    ) -> Box<dyn Future<Item = (), Error = ()> + Send> {
        let store = self.store.clone();
        let tx_hash = transfer.tx_hash;
        let self_clone = self.clone();
        let addr = Addresses {
            own_address: transfer.from,
            token_address: Some(token_address),
        };
        let amount = transfer.amount;
        Box::new(store
            .check_if_tx_processed(tx_hash)
            .map_err(move |_| error!("Error when querying store about transaction: {:?}", tx_hash))
            .and_then(move |processed| {
                if !processed {
                    Either::A(
                        store
                            .load_account_id_from_address(addr)
                            .and_then(move |id| {
                                debug!("Notifying connector about incoming ERC20 transaction for account {} for amount: {} (tx hash: {})", id, amount, tx_hash);
                                self_clone.notify_connector(id.to_string(), amount.to_string(), tx_hash)
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
            }))
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
            .and_then(|_| Ok(()))
    }

    fn notify_eth_transfer(&self, tx_hash: H256) -> Box<dyn Future<Item = (), Error = ()> + Send> {
        let our_address = self.address.own_address;
        let web3 = self.web3.clone();
        let store = self.store.clone();
        let self_clone = self.clone();
        // Skip transactions which have already been processed by the connector
        Box::new(store.check_if_tx_processed(tx_hash)
        .map_err(move |_| error!("Error when querying store about transaction: {:?}", tx_hash))
        .and_then(move |processed| {
            if !processed {
                Either::A(web3.eth().transaction(TransactionId::Hash(tx_hash))
                .map_err(move |err| error!("Could not fetch transaction data from transaction hash: {:?}. Got error: {:?}", tx_hash, err))
                .and_then(move |maybe_tx| {
                    // Unlikely to error out since we only call this on
                    // transaction hashes which were inside a mined block that
                    // had many confirmations
                    if let Some(tx) = maybe_tx { ok(tx) } else { err(())}
                })
                .and_then(move |tx| {
                    if let Some((from, amount)) = sent_to_us(tx, our_address) {
                        trace!("Got transaction for our account from {} for amount {}", from, amount);
                    if amount > U256::from(0) {
                        // if the tx was for us and had some non-0 amount, then let
                        // the connector know
                        let addr = Addresses {
                            own_address: from,
                            token_address: None,
                        };

                        return Either::A(store.load_account_id_from_address(addr)
                        .and_then(move |id| {
                            self_clone.notify_connector(id.to_string(), amount.to_string(), tx_hash)
                        })
                        .and_then(move |_| {
                            // only save the transaction hash if the connector
                            // was successfully notified
                            store.mark_tx_processed(tx_hash)
                                }));

                            }
                        }
                        // Ignore this transaction if it wasn't for us or was for a zero amount
                        Either::B(ok(()))
                }))
            } else {
                Either::B(ok(())) // return an empty future otherwise since we want to skip this transaction
            }
        }))
    }

    fn notify_connector(
        &self,
        account_id: String,
        amount: String,
        tx_hash: H256,
    ) -> impl Future<Item = (), Error = ()> {
        let engine_scale = self.asset_scale;
        let mut url = self.connector_url.clone();
        url.path_segments_mut()
            .expect("Invalid connector URL")
            .push("accounts")
            .push(&account_id.clone())
            .push("settlements");
        debug!("Making POST to {:?} {:?} about {:?}", url, amount, tx_hash);

        // settle for amount + uncredited_settlement_amount
        let account_id_clone = account_id.clone();
        let full_amount_fut = self
            .store
            .load_uncredited_settlement_amount(account_id.clone())
            .and_then(move |uncredited_settlement_amount| {
                let full_amount_fut2 = result(BigUint::from_str(&amount).map_err(move |err| {
                    let error_msg = format!("Error converting to BigUint {:?}", err);
                    error!("{:?}", error_msg);
                }))
                .and_then(move |amount| {
                    debug!("Loaded uncredited amount {}", uncredited_settlement_amount);
                    let full_amount = amount + uncredited_settlement_amount;
                    debug!(
                        "Notifying accounting system about full amount: {}",
                        full_amount
                    );
                    ok(full_amount)
                });
                ok(full_amount_fut2)
            })
            .flatten();

        let self_clone = self.clone();
        let ping_connector_fut = full_amount_fut.and_then(move |full_amount| {
            let url = url.clone();
            let account_id = account_id_clone.clone();
            let account_id2 = account_id_clone.clone();
            let full_amount2 = full_amount.clone();

            let action = move || {
                let client = Client::new();
                let account_id = account_id.clone();
                let full_amount = full_amount.clone();
                let full_amount_clone = full_amount.clone();
                client
                    .post(url.as_ref())
                    .header("Idempotency-Key", tx_hash.to_string())
                    .json(&json!(Quantity::new(full_amount.clone(), engine_scale)))
                    .send()
                    .map_err(move |err| {
                        error!(
                            "Error notifying Accounting System's account: {:?}, amount: {:?}: {:?}",
                            account_id, full_amount_clone, err
                        );
                    })
                    .and_then(move |ret| {
                        ok((ret, full_amount))
                    })
            };
            Retry::spawn(
                ExponentialBackoff::from_millis(10).take(MAX_RETRIES),
                action,
            )
            .map_err(move |_| {
                error!("Exceeded max retries when notifying connector about account {:?} for amount {:?} and transaction hash {:?}. Please check your API.", account_id2, full_amount2, tx_hash)
            })
        });

        ping_connector_fut.and_then(move |ret| {
            trace!("Accounting system responded with {:?}", ret.0);
            self_clone.process_connector_response(account_id, ret.0, ret.1)
        })
    }

    /// Parses a response from a connector into a Quantity type and calls a
    /// function to further process the parsed data to check if the store's
    /// uncredited settlement amount should be updated.
    fn process_connector_response(
        &self,
        account_id: String,
        response: HttpResponse,
        engine_amount: BigUint,
    ) -> Box<dyn Future<Item = (), Error = ()> + Send> {
        let self_clone = self.clone();
        if !response.status().is_success() {
            return Box::new(err(()));
        }
        Box::new(
            response
                .into_body()
                .concat2()
                .map_err(|err| {
                    let err = format!("Couldn't retrieve body {:?}", err);
                    error!("{}", err);
                })
                .and_then(move |body| {
                    // Get the amount stored by the connector and
                    // check for any precision loss / overflow
                    serde_json::from_slice::<Quantity>(&body).map_err(|err| {
                        let err = format!("Couldn't parse body {:?} into Quantity {:?}", body, err);
                        error!("{}", err);
                    })
                })
                .and_then(move |quantity: Quantity| {
                    self_clone.process_received_quantity(account_id, quantity, engine_amount)
                }),
        )
    }

    // Normalizes a received Quantity object against the local engine scale, and
    // if the normalized value is less than what the engine originally sent, it
    // stores it as uncredited settlement amount in the store.
    fn process_received_quantity(
        &self,
        account_id: String,
        quantity: Quantity,
        engine_amount: BigUint,
    ) -> Box<dyn Future<Item = (), Error = ()> + Send> {
        let store = self.store.clone();
        let engine_scale = self.asset_scale;
        Box::new(
            result(BigUint::from_str(&quantity.amount))
                .map_err(|err| {
                    let error_msg = format!("Error converting to BigUint {:?}", err);
                    error!("{:?}", error_msg);
                })
                .and_then(move |connector_amount: BigUint| {
                    // Scale the amount settled by the
                    // connector back up to our scale
                    result(connector_amount.normalize_scale(ConvertDetails {
                        from: quantity.scale,
                        to: engine_scale,
                    }))
                    .and_then(move |scaled_connector_amount| {
                        debug!(
                            "Engine settled to connector {}. Connector settled {}. Will save the difference: {}",
                            engine_amount, scaled_connector_amount, engine_amount > scaled_connector_amount,
                        );
                        if engine_amount > scaled_connector_amount {
                            let diff = engine_amount - scaled_connector_amount;
                            // connector settled less than we
                            // instructed it to, so we must save
                            // the difference
                            Either::A(store.save_uncredited_settlement_amount(account_id, diff.clone())
                            .and_then(move |_| {
                                debug!("Saved uncredited settlement amount {}", diff);
                                Ok(())
                            }))
                        } else {
                            Either::B(ok(()))
                        }
                    })
                }),
        )
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
        let value = U256::from_str(&tx.value.to_string()).unwrap();
        let estimate_gas_destination = if let Some(token_address) = token_address {
            token_address
        } else {
            to
        };
        let gas_amount_fut = web3.eth().estimate_gas(
            CallRequest {
                to: estimate_gas_destination,
                from: None,
                gas: None,
                gas_price: None,
                value: Some(value),
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

                    trace!(
                        "Gas required for transaction: {}, gas price: {}",
                        data[1],
                        data[0]
                    );

                    let signed_tx = signer.sign_raw_tx(tx.clone(), chain_id); // 3
                    let action = move || {
                        trace!("Sending tx to Ethereum: {}", hex::encode(&signed_tx));
                        web3.eth() // 4
                            // TODO use send_transaction_with_confirmation
                            .send_raw_transaction(signed_tx.clone().into())
                            .map_err(|err| {
                                error!("Error sending transaction to Ethereum ledger: {:?}", err);
                                err
                            })
                    };
                    Retry::spawn(
                        ExponentialBackoff::from_millis(10).take(MAX_RETRIES),
                        action,
                    )
                    .map_err(move |_err| {
                        error!("Unable to submit tx to Ethereum ledger");
                    })
                    .and_then(move |tx_hash| {
                        debug!("Transaction submitted. Hash: {:?}", tx_hash);
                        // TODO make sure the transaction is actually received
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
    ) -> impl Future<Item = (String, Addresses), Error = String> {
        let store = self.store.clone();
        let addr = self.address;
        let account_id_clone = account_id.clone();
        store
            .load_account_addresses(vec![account_id.clone()])
            .map_err(move |_err| {
                let error_msg = format!("[{:?}] Error getting account: {}", addr, account_id_clone);
                error!("{}", error_msg);
                error_msg
            })
            .and_then(move |addresses| ok((account_id, addresses[0])))
    }
}

impl<S, Si, A> SettlementEngine for EthereumLedgerSettlementEngine<S, Si, A>
where
    S: EthereumStore<Account = A>
        + LeftoversStore<AssetType = BigUint>
        + Clone
        + Send
        + Sync
        + 'static,
    Si: EthereumLedgerTxSigner + Clone + Send + Sync + 'static,
    A: EthereumAccount<AccountId = String> + Clone + Send + Sync + 'static,
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
                .post(url.as_ref())
                .header("Content-Type", "application/octet-stream")
                .header("Idempotency-Key", idempotency_uuid.clone())
                .body(challenge.clone())
                .send()
        };

        Box::new(
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
                                    == &payment_details.to.own_address.as_bytes()[..]
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
                            let data = HashMap::from_iter(vec![(account_id, payment_details.to)]);
                            store.save_account_addresses(data).map_err(move |err| {
                                let err = format!("Couldn't connect to store {:?}", err);
                                error!("{}", err);
                                (StatusCode::from_u16(500).unwrap(), err)
                            })
                        })
                })
            })
            .and_then(move |_| Ok((StatusCode::from_u16(201).unwrap(), "CREATED".to_owned()))),
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
                let error_msg = format!("Error converting to BigUint {:?}", err);
                error!("{:?}", error_msg);
                (StatusCode::from_u16(400).unwrap(), error_msg)
            }))
            .and_then(move |amount_from_connector| {
                // If we receive a Quantity { amount: "1", scale: 9},
                // we must normalize it to our engine's scale
                let amount = amount_from_connector.normalize_scale(ConvertDetails {
                    from: body.scale,
                    to: engine_scale,
                });

                result(amount)
                    .map_err(move |err| {
                        let error_msg = format!("Error scaling amount: {:?}", err);
                        error!("{:?}", error_msg);
                        (StatusCode::from_u16(400).unwrap(), error_msg)
                    })
                    .and_then(move |amount| {
                        // Typecast from num_bigint::BigUInt because we're using
                        // ethereum_types::U256 for all rust-web3 related functionality
                        result(U256::from_dec_str(&amount.to_string()).map_err(move |err| {
                            let error_msg = format!("Error converting to U256 {:?}", err);
                            error!("{:?}", error_msg);
                            (StatusCode::from_u16(400).unwrap(), error_msg)
                        }))
                    })
            })
            .and_then(move |amount| {
                self_clone
                    .load_account(account_id)
                    .map_err(move |err| {
                        let error_msg = format!("Error loading account {:?}", err);
                        error!("{}", error_msg);
                        (StatusCode::from_u16(400).unwrap(), error_msg)
                    })
                    .and_then(move |(account_id, addresses)| {
                        debug!("Sending settlement to account {} (Ethereum address: {}) for amount: {}{}",
                            account_id,
                            addresses.own_address,
                            amount,
                            if let Some(token_address) = addresses.token_address {
                                format!(" (token address: {}", token_address)
                            } else {
                                "".to_string()
                            });
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
    http_address: SocketAddr,
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
    let redis_uri = redis_uri.into_connection_info().unwrap();

    EthereumLedgerRedisStoreBuilder::new(redis_uri.clone())
        .connect()
        .and_then(move |ethereum_store| {
            let engine =
                EthereumLedgerSettlementEngineBuilder::new(ethereum_store.clone(), private_key)
                    .ethereum_endpoint(&ethereum_endpoint)
                    .chain_id(chain_id)
                    .connector_url(&connector_url)
                    .confirmations(confirmations)
                    .asset_scale(asset_scale)
                    .poll_frequency(poll_frequency)
                    .watch_incoming(watch_incoming)
                    .token_address(token_address)
                    .connect();

            let listener = TcpListener::bind(&http_address)
                .expect("Unable to bind to Settlement Engine address");
            let api = SettlementEngineApi::new(engine, ethereum_store);
            tokio::spawn(api.serve(listener.incoming()));
            info!("Ethereum Settlement Engine listening on: {}", http_address);
            Ok(())
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engines::ethereum_ledger::test_helpers::{
        block_on,
        fixtures::{ALICE, BOB, MESSAGES_API},
        test_engine, test_store, TestAccount,
    };
    use lazy_static::lazy_static;
    use mockito;

    lazy_static! {
        pub static ref ALICE_PK: String =
            String::from("380eb0f3d505f087e438eca80bc4df9a7faa24f868e69fc0440261a0fc0567dc");
        pub static ref BOB_PK: String =
            String::from("cc96601bc52293b53c4736a12af9130abf347669b3813f9ec4cafdf6991b087e");
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
        let engine = test_engine(
            store.clone(),
            ALICE_PK.clone(),
            0,
            &connector_url,
            None,
            false,
        );

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
            None,
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

    #[test]
    fn saves_leftovers() {
        // dummy tx_hashes to avoid making idempotent calls
        let tx_hash1 =
            H256::from_str("5ad3b56557dab5994c264ca17e2e08816341be2e6649ee6b2b1141006bfd347e")
                .unwrap();
        let tx_hash2 =
            H256::from_str("5ad3b56557dab5994c264ca17e2e08816341be2e6649ee6b2b1141006bfd3472")
                .unwrap();
        let tx_hash3 =
            H256::from_str("5ad3b56557dab5994c264ca17e2e08816341be2e6649ee6b2b1141006bfd3471")
                .unwrap();

        let bob = BOB.clone();
        let alice = ALICE.clone();
        let bob_store = test_store(bob.clone(), false, false, true);
        bob_store
            .save_account_addresses(HashMap::from_iter(vec![(
                "42".to_string(),
                Addresses {
                    own_address: alice.address,
                    token_address: None,
                },
            )]))
            .wait()
            .unwrap();

        // helper for customizing connector return values and making sure the
        // engine makes POSTs with the correct arguments
        let connector_mock = |received, ret| {
            // the connector will return any amount it receives with 2 less 0s
            let received = json!(Quantity::new(received, 18)).to_string();
            let ret = json!(Quantity::new(ret, 16)).to_string();
            mockito::mock("POST", "/accounts/42/settlements")
                .match_body(mockito::Matcher::JsonString(received))
                .with_status(200)
                .with_body(ret)
        };

        let full_amount = "100000000000";
        let full_return = "1000000000";
        let amount_with_leftovers = "110000000000";
        let full_return_with_leftovers = "1100000000";
        let partial_return = "900000000";

        // Bob's connector is initially set up to not return the full amount
        let bob_connector = connector_mock(full_amount, partial_return).create();
        // initialize the engine
        let bob_connector_url = mockito::server_url();
        let bob_engine = test_engine(
            bob_store.clone(),
            BOB_PK.clone(),
            0,
            &bob_connector_url,
            None,
            false,
        );

        // the call the engine makes when it picks up an on-chain event
        let ping_connector = |idempotency| {
            block_on(bob_engine.notify_connector(
                "42".to_string(),
                full_amount.to_string(),
                idempotency,
            ))
            .unwrap()
        };

        ping_connector(tx_hash1);
        bob_connector.assert();

        // for whatever reason, the connector now will return the full amount
        // (but our engine must also send the uncredited amount from the
        // previous call)
        let bob_connector =
            connector_mock(amount_with_leftovers, full_return_with_leftovers).create();
        ping_connector(tx_hash2);
        bob_connector.assert();

        // after the leftovers have been cleared, our engine continues normally
        let bob_connector = connector_mock(full_amount, full_return).create();
        ping_connector(tx_hash3);
        bob_connector.assert();
    }
}
