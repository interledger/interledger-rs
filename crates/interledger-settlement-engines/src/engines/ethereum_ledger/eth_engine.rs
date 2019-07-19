use super::types::{Addresses, EthereumAccount, EthereumLedgerTxSigner, EthereumStore};
use super::utils::make_tx;
use log::{debug, error, trace};
use std::collections::HashMap;
use std::iter::FromIterator;

use ethereum_tx_sign::web3::{
    api::Web3,
    futures::future::{ok, result, Either, Future},
    futures::stream::Stream,
    transports::Http,
    types::{Address, BlockNumber, TransactionId, H256, U256},
};
use hyper::StatusCode;
use reqwest::r#async::{Client, Response as HttpResponse};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::{
    marker::PhantomData,
    str::FromStr,
    time::{Duration, Instant},
};
use tokio::timer::Interval;
use tokio_retry::{strategy::ExponentialBackoff, Retry};
use url::Url;
use uuid::Uuid;

use crate::{ApiResponse, Quantity, SettlementEngine};

const MAX_RETRIES: usize = 10;

#[derive(Debug, Clone, Serialize, Deserialize)]
enum MessageType {
    PaymentDetailsRequest = 0,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ReceiveMessageDetails {
    msg_type: MessageType,
    data: Vec<u8>,
}

impl ReceiveMessageDetails {
    fn new_payment_details_request() -> Self {
        ReceiveMessageDetails {
            msg_type: MessageType::PaymentDetailsRequest,
            data: vec![],
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PaymentDetailsResponse {
    to: Addresses,
    tag: Uuid,
}

impl PaymentDetailsResponse {
    fn new(to: Addresses) -> Self {
        PaymentDetailsResponse {
            to,
            tag: Uuid::new_v4(),
        }
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
/// that all transactions that get credited to the connector have sufficient
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
    speculative_nonce_middleware: SpeculativeNonceMiddleware,
}

#[derive(Clone, Debug)]
struct SpeculativeNonceMiddleware {
    web3: Web3<Http>,
    own_address: Address,
    latest_confirmed_nonce: u64,
}

impl SpeculativeNonceMiddleware {
    fn new(web3: Web3<Http>, own_address: Address) -> Self {
        Self {
            web3,
            own_address,
            latest_confirmed_nonce: 0,
        }
    }

    fn next_nonce(&self) -> impl Future<Item = U256, Error = ()> {
        // TODO: Implement speculative logic.
        self.web3
            .eth()
            .transaction_count(self.own_address, None)
            .map_err(|err| error!("Couldn't fetch nonce: {:?}", err))
    }
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

    pub fn chain_id(&mut self, chain_id: u8) -> &mut Self {
        self.chain_id = Some(chain_id);
        self
    }

    pub fn confirmations(&mut self, confirmations: u8) -> &mut Self {
        self.confirmations = Some(confirmations);
        self
    }

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

        let (eloop, transport) = Http::new(ethereum_endpoint).unwrap();
        eloop.into_remote();
        let web3 = Web3::new(transport);
        let address = Addresses {
            own_address: self.signer.address(),
            token_address: self.token_address,
        };
        let speculative_nonce_middleware =
            SpeculativeNonceMiddleware::new(web3.clone(), address.own_address);

        let engine = EthereumLedgerSettlementEngine {
            web3,
            store: self.store.clone(),
            signer: self.signer.clone(),
            address,
            chain_id,
            confirmations,
            poll_frequency,
            connector_url,
            speculative_nonce_middleware,
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
    fn notify_connector_on_incoming_settlement(&self) {
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
                        debug!(
                            "[{:?}] Getting settlement data from the blockchain; instant={:?}",
                            address, instant
                        );
                        tokio::spawn(_self.notifier());
                        Ok(())
                    })
                    .map_err(|e| panic!("interval errored; err={:?}", e)),
            );
        });
    }

    /// Routine for notifying the connector about incoming transactions.
    /// Algorithm (heavily unoptimized):
    /// 1. Fetch the last observed block number and account balance from the store
    /// 2. Fetch the current block number from Ethereum
    /// 3. Check the current balance of the account
    /// 4. If the balance is not larger than the last observed balance, terminate.
    /// 5. Fetch all blocks since the last observed one,
    ///    until $(current block number - confirmations), where $confirmations
    ///    is a security parameter to be safe against block reorgs.
    /// 6. For each block (in parallel):
    ///     For each transaction (in parallel):
    ///     1. Skip if it is not sent to our address or have 0 value.
    ///     2. Fetch the id that matches its sender from the store
    ///     3. Notify the connector about it by making a POST request to the connector's
    ///        /accounts/$id/settlement endpoint with transaction's value as the
    ///        body. This call is retried if it fails.
    /// 7. Save the (current block number - confirmations) and current account
    ///    balance to the store, to be used as last observed data for the next
    ///    call of this function.
    fn notifier(&self) -> impl Future<Item = (), Error = ()> + Send {
        let confirmations = self.confirmations;
        let our_address = self.address.own_address;
        let connector_url = self.connector_url.clone();
        let web3 = self.web3.clone();
        let store = self.store.clone();
        let store_clone = self.store.clone();

        store.load_recently_observed_data()
        .and_then(move |(last_observed_block, last_observed_balance)| {
            web3
                .eth()
                .block_number()
                .map_err(|err| println!("Got error {:?}", err))
                .and_then(move |current_block| {
                    debug!("Current block {}", current_block);
                    let fetch_until = current_block - confirmations;
                    ok(fetch_until)
                })
                .and_then(move |fetch_until| {
                    debug!("Will fetch txs from block {} until {}", last_observed_block, fetch_until);
                    web3
                        .eth()
                        .balance(our_address, None)
                        .map_err(|err| error!("Got error while getting balance {:?}", err))
                        .and_then({let web3 = web3.clone(); move |new_balance| {
                            if new_balance > last_observed_balance {
                                // iterate over all blocks 
                                for block_num in last_observed_block.low_u64()..=fetch_until.low_u64() {
                                    let web3 = web3.clone();
                                    trace!("Getting block {}", block_num);
                                    let block_url = connector_url.clone();
                                    let get_blocks_future = web3.eth().block(BlockNumber::Number(block_num as u64).into())
                                    .map_err(move |err| error!("Got error while getting block {}: {:?}", block_num, err))
                                    .and_then({let store_clone = store_clone.clone(); move |block| {
                                        trace!("Fetched block: {:?}", block_num);
                                        if let Some(block) = block {
                                            // iterate over all tx_hashes in the block
                                            // and spawn a future that will execute
                                            // the settlement request if the
                                            // transaction is sent to us and has
                                            // >0 value
                                            for tx_hash in block.transactions {
                                                trace!("Got transactions {:?}", tx_hash);
                                                let store_clone = store_clone.clone();
                                                let mut url = block_url.clone();
                                                let web3_clone = web3.clone();
                                                let settle_tx_future = store_clone.check_tx_credited(tx_hash)
                                                    .map_err(move |_| error!("Transaction {} has already been credited!", tx_hash))
                                                    .and_then(move |credited| {
                                                        if credited {
                                                            Either::A(ok(()))
                                                        } else {
                                                Either::B(web3_clone.eth().transaction(TransactionId::Hash(tx_hash))
                                                .map_err(move |err| error!("Transaction {} was not submitted to the connector. Please submit it manually. Got error: {:?}", tx_hash, err))
                                                .and_then(move |tx| {
                                                    let tx = tx.clone();
                                                    if let Some(tx) = tx {
                                                        if let Some(to) = tx.to {
                                                            if tx.value > U256::from(0) {
                                                                trace!("Forwarding transaction: {:?}", tx);
                                                                // Optimization: This will make 1 settlement API request to
                                                                // the connector per transaction that comes to us. Since
                                                                // we're scanning across multiple blocks, we want to
                                                                // optimize it so that: it gathers a mapping of
                                                                // (accountId=>amount) across all transactions and blocks, and
                                                                // only makes 1 transaction per accountId with the
                                                                // appropriate amount.
                                                                if to == our_address {
                                                                    let addr = Addresses {
                                                                        own_address: tx.from,
                                                                        token_address: None,
                                                                    };
                                                                    let notify_connector_future = store_clone.load_account_id_from_address(addr).and_then(move |id| {
                                                                        url.path_segments_mut()
                                                                            .expect("Invalid connector URL")
                                                                            .push("accounts")
                                                                            .push(&id.to_string())
                                                                            .push("settlement");
                                                                        let client = Client::new();
                                                                        let idempotency_uuid = uuid::Uuid::new_v4().to_hyphenated().to_string();
                                                                        // retry on fail - https://github.com/ihrwein/backoff
                                                                        debug!("Making POST to {:?} {:?} {:?}", url, tx.value.to_string(), idempotency_uuid);
                                                                        let action = move || {
                                                                            client.post(url.clone())
                                                                            .header("Idempotency-Key", idempotency_uuid.clone())
                                                                            .json(&json!({"amount" : tx.value.low_u64()}))
                                                                            .send()
                                                                            .map_err(move |err| println!("Error notifying accounting system about transaction {:?}: {:?}", tx_hash, err))
                                                                            .and_then(move |response| {
                                                                                trace!("Accounting system responded with {:?}", response);
                                                                                if response.status().is_success() {
                                                                                    Ok(())
                                                                                } else {
                                                                                    Err(())
                                                                                }
                                                                            })
                                                                        };
                                                                        tokio::spawn(Retry::spawn(ExponentialBackoff::from_millis(10).take(MAX_RETRIES), action).map_err(|_| error!("Failed to keep retrying!")));
                                                                        ok(())
                                                                    });
                                                                    tokio::spawn(notify_connector_future);
                                                                }
                                                            }
                                                        }
                                                    }
                                                    ok(())
                                                }))
                                                        }
                                            });
                                                // spawn a future so that we can
                                                // process as many as possible in parallel
                                                tokio::spawn(settle_tx_future);
                                            }
                                        } else {
                                            println!("Block {:?} not found", block_num);
                                        }
                                        ok(())
                                    }});
                                    tokio::spawn(get_blocks_future);
                                }
                            }
                            // Assume optimistically that the function will succeed
                            // everywhere and save the current block as the latest
                            // block. We can implement retry logic for blocks/txs that
                            // are not fetched, OR process them serially, and on failure
                            // save the block and quit.
                            tokio::spawn(store.save_recently_observed_data(fetch_until, new_balance))
                        }})
                })
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
        let nonce_middleware = self.speculative_nonce_middleware.clone();
        let own_address = self.address.own_address;
        let chain_id = self.chain_id;
        let signer = self.signer.clone();
        Box::new(
            nonce_middleware
                .next_nonce()
                .map_err(move |err| {
                    error!(
                        "Error when querying account {:?} nonce: {:?}",
                        own_address, err
                    )
                })
                .and_then(move |nonce| {
                    let tx = make_tx(to, amount, nonce, token_address); // 2
                    let signed_tx = signer.sign(tx, chain_id); // 3
                    debug!("Sending transaction: {:?}", signed_tx);
                    web3.eth() // 4
                        .send_raw_transaction(signed_tx.clone().into())
                        .map_err(move |err| {
                            error!("Error when sending transaction {:?}: {:?}", signed_tx, err)
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
        account_id: String,
    ) -> Box<dyn Future<Item = ApiResponse, Error = ApiResponse> + Send> {
        let self_clone = self.clone();
        let store: S = self.store.clone();

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
                let req = ReceiveMessageDetails::new_payment_details_request();
                let body = serde_json::to_vec(&req).unwrap();
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
                        .body(body.clone())
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
                        let data = HashMap::from_iter(vec![(account_id, payment_details.to)]);
                        store.save_account_addresses(data).map_err(move |err| {
                            let err = format!("Couldn't connect to store {:?}", err);
                            error!("{}", err);
                            (StatusCode::from_u16(500).unwrap(), err)
                        })
                    })
                })
                .and_then(move |_| Ok((StatusCode::from_u16(201).unwrap(), "CREATED".to_owned())))
            }),
        )
    }

    /// Settlement Engine's function that corresponds to the
    /// /accounts/:id/messages endpoint (POST). Responds depending on message type
    /// which is parsed from the request's body:
    /// - PaymentDetailsRequest: returns the SE's Ethereum & Token Addresses
    /// - more request types to be potentially added in the future
    fn receive_message(
        &self,
        account_id: String,
        body: Vec<u8>,
    ) -> Box<dyn Future<Item = ApiResponse, Error = ApiResponse> + Send> {
        let address = self.address;
        Box::new(
            result(serde_json::from_slice(&body))
                .map_err(move |err| {
                    let error_msg = format!("Could not parse message body: {:?}", err);
                    error!("{}", error_msg);
                    (StatusCode::from_u16(400).unwrap(), error_msg)
                })
                .and_then(move |message: ReceiveMessageDetails| {
                    // We are only returning our information, so
                    // there is no need to return any data about the
                    // provided account.
                    debug!(
                        "Responding with our account's details {} {:?}",
                        account_id, address
                    );
                    let resp = match message.msg_type {
                        MessageType::PaymentDetailsRequest => {
                            let ret = PaymentDetailsResponse::new(address);
                            serde_json::to_string(&ret).unwrap()
                        }
                    };
                    Ok((StatusCode::from_u16(200).unwrap(), resp))
                }),
        )
    }
    /// Settlement Engine's function that corresponds to the
    /// /accounts/:id/settlement endpoint (POST). It performs an Ethereum
    /// onchain transaction to the Ethereum Address that corresponds to the
    /// provided account id, for the amount specified in the message's body. If
    /// the account is associated with an ERC20 token, it makes an ERC20 call instead.
    fn send_money(
        &self,
        account_id: String,
        body: Quantity,
    ) -> Box<dyn Future<Item = ApiResponse, Error = ApiResponse> + Send> {
        let amount = U256::from(body.amount);
        let self_clone = self.clone();

        Box::new(
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
                .and_then(move |_| Ok((StatusCode::OK, "OK".to_string()))),
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

        let _bob_mock = mockito::mock("POST", "/accounts/42/settlement")
            .match_body(mockito::Matcher::Regex(r"amount=\d*".to_string()))
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

        let ret = block_on(alice_engine.send_money(bob.id.to_string(), Quantity { amount: 100 }))
            .unwrap();
        assert_eq!(ret.0.as_u16(), 200);
        assert_eq!(ret.1, "OK");

        std::thread::sleep(Duration::from_millis(100)); // wait a few seconds so that the receiver's engine that does the polling
        ganache_pid.kill().unwrap(); // kill ganache since it's no longer needed
    }

    #[test]
    fn test_create_get_account() {
        let bob: TestAccount = BOB.clone();

        let body_se_data = serde_json::to_string(&PaymentDetailsResponse {
            to: Addresses {
                own_address: bob.address,
                token_address: None,
            },
            tag: Uuid::new_v4(),
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

        let store = test_store(bob.clone(), false, false, false);
        let engine = test_engine(store.clone(), ALICE_PK.clone(), 0, &connector_url, false);

        let ret = block_on(engine.create_account(bob.id.to_string())).unwrap();
        assert_eq!(ret.0.as_u16(), 201);
        assert_eq!(ret.1, "CREATED");

        m.assert();
    }

    #[test]
    fn test_receive_message() {
        let bob: TestAccount = BOB.clone();

        let body_se_data = serde_json::to_string(&PaymentDetailsResponse {
            to: Addresses {
                own_address: bob.address,
                token_address: Some(bob.token_address),
            },
            tag: Uuid::new_v4(),
        })
        .unwrap();

        let m = mockito::mock("POST", MESSAGES_API.clone())
            .with_status(200)
            .with_body(body_se_data)
            .expect(1) // only 1 request is made to the connector (idempotency works properly)
            .create();
        let connector_url = mockito::server_url();

        let store = test_store(ALICE.clone(), false, false, false);
        let engine = test_engine(store.clone(), ALICE_PK.clone(), 0, &connector_url, false);

        let ret = block_on(engine.create_account(bob.id.to_string())).unwrap();
        assert_eq!(ret.0.as_u16(), 201);
        assert_eq!(ret.1, "CREATED");

        let receive_message = ReceiveMessageDetails::new_payment_details_request();
        let receive_message = serde_json::to_vec(&receive_message).unwrap();
        let ret = block_on(engine.receive_message(bob.id.to_string(), receive_message)).unwrap();
        assert_eq!(ret.0.as_u16(), 200);

        // the data our SE will return to the node must match alice's addrs
        let alice_addrs = Addresses {
            own_address: ALICE.address,
            token_address: None,
        };
        let data: PaymentDetailsResponse = serde_json::from_str(&ret.1).unwrap();
        assert_eq!(data.to, alice_addrs);
        m.assert();
    }
}
