use super::types::{Addresses, EthereumAccount, EthereumLedgerTxSigner, EthereumStore};
use super::utils::make_tx;
use log::{debug, error};

use bytes::Bytes;
use ethereum_tx_sign::web3::{
    api::Web3,
    futures::future::{ok, result, Either, Future},
    futures::stream::Stream,
    transports::Http,
    types::{Address, BlockNumber, TransactionId, H256, U256},
};
use hyper::{Response, StatusCode};
use interledger_settlement::{IdempotentStore, SettlementData};
use reqwest::r#async::{Client, Response as HttpResponse};
use ring::digest::{digest, SHA256};
use serde::{Deserialize, Serialize};
use std::{
    marker::PhantomData,
    str::FromStr,
    time::{Duration, Instant},
};
use tokio::timer::Interval;
use tokio_executor::spawn;
use url::Url;
use uuid::Uuid;

use crate::SettlementEngine;

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
    tag: String,
}

impl PaymentDetailsResponse {
    fn new(to: Addresses) -> Self {
        PaymentDetailsResponse {
            to,
            tag: Uuid::new_v4().to_hyphenated().to_string(),
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
    confirmations: usize,
    poll_frequency: Duration,
    connector_url: Url,
}

impl<S, Si, A> EthereumLedgerSettlementEngine<S, Si, A>
where
    S: EthereumStore<Account = A> + IdempotentStore + Clone + Send + Sync + 'static,
    Si: EthereumLedgerTxSigner + Clone + Send + Sync + 'static,
    A: EthereumAccount + Send + Sync + 'static,
{
    #[allow(clippy::all)]
    pub fn new(
        endpoint: String,
        store: S,
        signer: Si,
        chain_id: u8,
        confirmations: usize,
        poll_frequency: Duration,
        connector_url: Url,
        token_address: Option<Address>,
        watch_incoming: bool, // If set to false, the engine will not poll for incoming transactions, and it's expeccted to be done out of process.
    ) -> Self {
        let (eloop, transport) = Http::new(&endpoint).unwrap();
        eloop.into_remote();
        let web3 = Web3::new(transport);
        let address = Addresses {
            own_address: signer.address(),
            token_address,
        };
        let engine = EthereumLedgerSettlementEngine {
            web3,
            store,
            signer,
            address,
            chain_id,
            confirmations,
            poll_frequency,
            connector_url,
            account_type: PhantomData,
        };
        if watch_incoming {
            engine.notify_connector_on_incoming_settlement();
        }
        engine
    }

    /// Periodically spawns a job every `self.poll_frequency` that notifies the
    /// Settlement Engine's connectors about transactions which are sent to the
    /// engine's address.
    fn notify_connector_on_incoming_settlement(&self) {
        let _self = self.clone();
        let interval = self.poll_frequency;
        let address = self.address;
        debug!(
            "[{:?}]SE service for listening to incoming settlements. Interval: {:?}",
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
                                for block_num in last_observed_block.low_u64()..fetch_until.low_u64() {
                                    let web3 = web3.clone();
                                    debug!("Getting block {}", block_num);
                                    let block_url = connector_url.clone();
                                    let get_blocks_future = web3.eth().block(BlockNumber::Number(block_num as u64).into())
                                    .map_err(move |err| error!("Got error while getting block {}: {:?}", block_num, err))
                                    .and_then({let store_clone = store_clone.clone(); move |block| {
                                        debug!("Fetched block: {:?}", block_num);
                                        if let Some(block) = block {
                                            // iterate over all tx_hashes in the block
                                            // and spawn a future that will execute
                                            // the settlement request if the
                                            // transaction
                                            for tx_hash in block.transactions {
                                                debug!("Got transactions {:?}", tx_hash);
                                                let store_clone = store_clone.clone();
                                                let mut url = block_url.clone();
                                                let settle_tx_future = web3.eth().transaction(TransactionId::Hash(tx_hash))
                                                .map_err(move |err| error!("Got error while getting transaction {}: {:?}", block_num, err))
                                                .and_then(move |tx| {
                                                    let tx = tx.clone();
                                                    if let Some(tx) = tx {
                                                        if let Some(to) = tx.to {
                                                            if tx.value > U256::from(0) {
                                                                debug!("Forwarding transaction: {:?}", tx);
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
                                                                        let client = reqwest::r#async::Client::new();
                                                                        let idempotency_uuid = uuid::Uuid::new_v4().to_hyphenated().to_string();
                                                                        // retry on fail - https://github.com/ihrwein/backoff
                                                                        debug!("Making POST to {:?} {:?} {:?}", url, tx.value.to_string(), idempotency_uuid);
                                                                        let mut params = std::collections::HashMap::new();
                                                                        params.insert("amount", tx.value.low_u64());
                                                                        client.post(url)
                                                                            .header("Idempotency-Key", idempotency_uuid)
                                                                            .form(&params)
                                                                            .send()
                                                                            .map_err(move |err| println!("Error notifying accounting system about transaction {:?}: {:?}", tx_hash, err))
                                                                            .and_then(move |response| {
                                                                                debug!("Accounting system responded with {:?}", response);
                                                                                if response.status().is_success() {
                                                                                    Ok(())
                                                                                } else {
                                                                                    Err(())
                                                                                }
                                                                            })
                                                                    });
                                                                    tokio::spawn(notify_connector_future);
                                                                }
                                                            }
                                                        }
                                                    }
                                                    ok(())
                                                });

                                                // spawn a future so that we can do
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
        let own_address = self.address.own_address;
        let chain_id = self.chain_id;
        let signer = self.signer.clone();
        Box::new(
            web3.eth()
                .transaction_count(own_address, None) // 1
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

    #[allow(unused)]
    fn get_account(
        &self,
        account_id: String,
    ) -> impl Future<Item = Response<String>, Error = Response<String>> {
        self.load_account(account_id)
            .map_err(|err| {
                let error_msg = format!("Error loading account {:?}", err);
                error!("{}", error_msg);
                Response::builder().status(400).body(error_msg).unwrap()
            })
            .and_then(move |(_account_id, addresses)| {
                let body = serde_json::to_string(&addresses).unwrap();
                Ok(Response::builder().status(200).body(body).unwrap())
            })
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

    /// Helper function that returns any idempotent data that corresponds to a
    /// provided idempotency key. It fails if the hash of the input that
    /// generated the idempotent data does not match the hash of the provided input.
    fn check_idempotency(
        &self,
        idempotency_key: Option<String>,
        input_hash: [u8; 32],
    ) -> impl Future<Item = Option<(StatusCode, Bytes)>, Error = (StatusCode, String)> {
        if let Some(idempotency_key) = idempotency_key {
            Either::A(
                self.store
                    .load_idempotent_data(idempotency_key.clone())
                    .map_err(move |err| {
                        let err = format!("Couldn't connect to store {:?}", err);
                        error!("{}", err);
                        (StatusCode::from_u16(500).unwrap(), err)
                    })
                    .and_then(move |ret: Option<(StatusCode, Bytes, [u8; 32])>| {
                        if let Some(d) = ret {
                            if d.2 != input_hash {
                                // Stripe CONFLICT status code
                                return Err((
                                    StatusCode::from_u16(409).unwrap(),
                                    "Provided idempotency key is tied to other input".to_string(),
                                ));
                            }
                            if d.0.is_success() {
                                return Ok(Some((d.0, d.1)));
                            } else {
                                return Err((d.0, String::from_utf8_lossy(&d.1).to_string()));
                            }
                        }
                        Ok(None)
                    }),
            )
        } else {
            Either::B(ok(None))
        }
    }
}

impl<S, Si, A> SettlementEngine for EthereumLedgerSettlementEngine<S, Si, A>
where
    S: EthereumStore<Account = A> + IdempotentStore + Clone + Send + Sync + 'static,
    Si: EthereumLedgerTxSigner + Clone + Send + Sync + 'static,
    A: EthereumAccount + Send + Sync + 'static,
{
    /// Settlement Engine's function that corresponds to the
    /// /accounts/:id/messages endpoint (POST). Responds depending on message type
    /// which is parsed from the request's body:
    /// - PaymentDetailsRequest: returns the SE's Ethereum & Token Addresses
    /// - more request types to be potentially added in the future
    /// Is idempotent.
    fn receive_message(
        &self,
        account_id: String,
        body: Vec<u8>,
        idempotency_key: Option<String>,
    ) -> Box<dyn Future<Item = Response<String>, Error = Response<String>> + Send> {
        let self_clone = self.clone();
        let input = format!("{}{:?}", account_id, body);
        let input_hash = get_hash_of(input.as_ref());

        // 1. check idempotency
        // 2. try parsing the body
        // 3. return the engine's address
        Box::new(
            self_clone
                .check_idempotency(idempotency_key.clone(), input_hash) // 1
                .map_err(|res| Response::builder().status(res.0).body(res.1).unwrap())
                .and_then(move |ret: Option<(StatusCode, Bytes)>| {
                    if let Some(d) = ret {
                        return Either::A(ok(Response::builder()
                            .status(d.0)
                            .body(String::from_utf8_lossy(&d.1).to_string())
                            .unwrap()));
                    }
                    Either::B(
                        result(serde_json::from_slice(&body)) // 2
                            .map_err(move |err| {
                                let error_msg = format!("Could not parse message body: {:?}", err);
                                error!("{}", error_msg);
                                Response::builder().status(400).body(error_msg).unwrap()
                            })
                            .and_then(move |message: ReceiveMessageDetails| {
                                // We are only returning our information, so
                                // there is no need to return any data about the
                                // provided account.
                                let resp = match message.msg_type {
                                    MessageType::PaymentDetailsRequest => {
                                        // 3
                                        let ret = PaymentDetailsResponse::new(self_clone.address);
                                        serde_json::to_string(&ret).unwrap()
                                    }
                                };
                                Ok(Response::builder().status(200).body(resp).unwrap())
                            }),
                    )
                }),
        )
    }

    /// Settlement Engine's function that corresponds to the
    /// /accounts/:id/ endpoint (POST). It queries the connector's
    /// accounts/:id/messages with a "PaymentDetailsRequest" that gets forwarded
    /// to the connector's peer that matches the provided account id, which
    /// finally gets forwarded to the peer SE's receive_message endpoint which
    /// responds with its Ethereum and Token addresses. Upon
    /// receival of Ethereum and Token addresses from the peer, it saves them in
    /// the store.
    /// Is idempotent.
    fn create_account(
        &self,
        account_id: String,
        idempotency_key: Option<String>,
    ) -> Box<dyn Future<Item = Response<String>, Error = Response<String>> + Send> {
        let self_clone = self.clone();
        let store: S = self.store.clone();
        let store_clone2 = self.store.clone();
        let input_hash = get_hash_of(account_id.as_ref());

        Box::new(
            self_clone
                .check_idempotency(idempotency_key.clone(), input_hash)
                .map_err(|res| Response::builder().status(res.0).body(res.1).unwrap())
                .and_then(move |ret: Option<(StatusCode, Bytes)>| {
                    if let Some(d) = ret {
                        return Either::A(ok(Response::builder()
                            .status(d.0)
                            .body(String::from_utf8_lossy(&d.1).to_string())
                            .unwrap()));
                    }
                    Either::B(
                        result(A::AccountId::from_str(&account_id).map_err({
                            let store = store.clone();
                            let idempotency_key = idempotency_key.clone();
                            move |_err| {
                                let error_msg = "Unable to parse account".to_string();
                                error!("{}", error_msg);
                                let status_code = StatusCode::from_u16(400).unwrap();
                                let data = Bytes::from(error_msg.clone());
                                if let Some(idempotency_key) = idempotency_key {
                                    spawn(store.save_idempotent_data(
                                        idempotency_key,
                                        input_hash,
                                        status_code,
                                        data,
                                    ));
                                }
                                Response::builder()
                                    .status(status_code)
                                    .body(error_msg)
                                    .unwrap()
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
                            client
                                .post(url)
                                .header("Idempotency-Key", idempotency_uuid)
                                .body(body)
                                .send()
                                .map_err(move |err| {
                                    let err = format!("Couldn't notify connector {:?}", err);
                                    error!("{}", err);
                                    Response::builder().status(500).body(err).unwrap()
                                })
                                .and_then(move |resp| {
                                    if let Some(idempotency_key) = idempotency_key {
                                        spawn(store_clone2.save_idempotent_data(
                                            idempotency_key,
                                            input_hash,
                                            StatusCode::from_u16(201).unwrap(),
                                            Bytes::from("CREATED"),
                                        ));
                                    }
                                    parse_body_into_payment_details(resp).and_then(
                                        move |payment_details| {
                                            store
                                                .save_account_addresses(
                                                    vec![account_id],
                                                    vec![payment_details.to],
                                                )
                                                .map_err(move |err| {
                                                    let err = format!(
                                                        "Couldn't connect to store {:?}",
                                                        err
                                                    );
                                                    error!("{}", err);
                                                    Response::builder()
                                                        .status(500)
                                                        .body(err)
                                                        .unwrap()
                                                })
                                        },
                                    )
                                })
                        })
                        .and_then(move |_| {
                            Ok(Response::builder()
                                .status(201)
                                .body("CREATED".to_string())
                                .unwrap())
                        }),
                    )
                }),
        )
    }

    /// Settlement Engine's function that corresponds to the
    /// /accounts/:id/settlement endpoint (POST). It performs an Ethereum
    /// onchain transaction to the Ethereum Address that corresponds to the
    /// provided account id, for the amount specified in the message's body. If
    /// the account is associated with an ERC20 token, it makes an ERC20 call instead.
    /// Is idempotent.
    fn send_money(
        &self,
        account_id: String,
        body: SettlementData,
        idempotency_key: Option<String>,
    ) -> Box<dyn Future<Item = Response<String>, Error = Response<String>> + Send> {
        let amount = U256::from(body.amount);
        let self_clone = self.clone();
        let store = self.store.clone();
        let store_clone = store.clone();
        let store_clone2 = store.clone();
        let idempotency_key_clone = idempotency_key.clone();
        let idempotency_key_clone2 = idempotency_key.clone();

        let input = format!("{}{:?}", account_id, body);
        let input_hash = get_hash_of(input.as_ref());

        Box::new(
            self.check_idempotency(idempotency_key.clone(), input_hash)
                .map_err(|res| Response::builder().status(res.0).body(res.1).unwrap())
                .and_then(move |ret: Option<(StatusCode, Bytes)>| {
                    if let Some(d) = ret {
                        return Either::A(ok(Response::builder()
                            .status(d.0)
                            .body(String::from_utf8_lossy(&d.1).to_string())
                            .unwrap()));
                    }
                    Either::B(
                        self_clone
                            .load_account(account_id)
                            .map_err(move |err| {
                                let error_msg = format!("Error loading account {:?}", err);
                                error!("{}", error_msg);
                                if let Some(idempotency_key) = idempotency_key {
                                    spawn(store.save_idempotent_data(
                                        idempotency_key,
                                        input_hash,
                                        StatusCode::from_u16(400).unwrap(),
                                        Bytes::from(error_msg.clone()),
                                    ));
                                }
                                Response::builder().status(400).body(error_msg).unwrap()
                            })
                            .and_then(move |(_account_id, addresses)| {
                                self_clone
                                    .settle_to(
                                        addresses.own_address,
                                        amount,
                                        addresses.token_address,
                                    )
                                    .map_err(move |_| {
                                        let error_msg =
                                            "Error connecting to the blockchain.".to_string();
                                        error!("{}", error_msg);
                                        if let Some(idempotency_key_clone) = idempotency_key_clone {
                                            spawn(store_clone.save_idempotent_data(
                                                idempotency_key_clone,
                                                input_hash,
                                                StatusCode::from_u16(502).unwrap(),
                                                Bytes::from(error_msg.clone()),
                                            ));
                                        }
                                        Response::builder().status(502).body(error_msg).unwrap()
                                    })
                            })
                            .and_then(move |_| {
                                if let Some(idempotency_key_clone2) = idempotency_key_clone2 {
                                    spawn(store_clone2.save_idempotent_data(
                                        idempotency_key_clone2,
                                        input_hash,
                                        StatusCode::from_u16(200).unwrap(),
                                        Bytes::from("OK".to_string()),
                                    ));
                                }
                                Ok(Response::builder()
                                    .status(200)
                                    .body("OK".to_string())
                                    .unwrap())
                            }),
                    )
                }),
        )
    }
}

fn parse_body_into_payment_details(
    resp: HttpResponse,
) -> impl Future<Item = PaymentDetailsResponse, Error = Response<String>> {
    resp.into_body()
        .concat2()
        .map_err(|err| {
            let err = format!("Couldn't retrieve body {:?}", err);
            error!("{}", err);
            Response::builder().status(500).body(err).unwrap()
        })
        .and_then(move |body| {
            serde_json::from_slice::<PaymentDetailsResponse>(&body).map_err(|err| {
                let err = format!(
                    "Couldn't parse body {:?} into payment details {:?}",
                    body, err
                );
                error!("{}", err);
                Response::builder().status(500).body(err).unwrap()
            })
        })
}

fn get_hash_of(preimage: &[u8]) -> [u8; 32] {
    let mut hash = [0; 32];
    hash.copy_from_slice(digest(&SHA256, preimage).as_ref());
    hash
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

    static IDEMPOTENCY: &str = "AJKJNUjM0oyiAN46";

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
            .save_account_addresses(
                vec![0],
                vec![Addresses {
                    own_address: bob.address,
                    token_address: None,
                }],
            )
            .wait()
            .unwrap();

        let bob_store = test_store(bob.clone(), false, false, true);
        bob_store
            .save_account_addresses(
                vec![42],
                vec![Addresses {
                    own_address: alice.address,
                    token_address: None,
                }],
            )
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
            bob_connector_url,
            true,
        );

        let alice_engine = test_engine(
            alice_store.clone(),
            ALICE_PK.clone(),
            0,
            "http://127.0.0.1:9999".to_owned(), // url does not matter here
            false, // alice sends the transaction to bob (set it up so that she doesn't listen for inc txs)
        );

        let ret: Response<_> = block_on(alice_engine.send_money(
            bob.id.to_string(),
            SettlementData { amount: 100 },
            Some(IDEMPOTENCY.to_string()),
        ))
        .unwrap();
        assert_eq!(ret.status().as_u16(), 200);
        assert_eq!(ret.body(), "OK");

        let ret: Response<_> = block_on(alice_engine.send_money(
            bob.id.to_string(),
            SettlementData { amount: 100 },
            Some(IDEMPOTENCY.to_string()),
        ))
        .unwrap();
        assert_eq!(ret.status().as_u16(), 200);
        assert_eq!(ret.body(), "OK");

        // fails with different id and same data
        let ret: Response<_> = block_on(alice_engine.send_money(
            "42".to_string(),
            SettlementData { amount: 100 },
            Some(IDEMPOTENCY.to_string()),
        ))
        .unwrap_err();
        assert_eq!(ret.status().as_u16(), 409);
        assert_eq!(
            ret.body(),
            "Provided idempotency key is tied to other input"
        );

        // fails with same id and different data
        let ret: Response<_> = block_on(alice_engine.send_money(
            bob.id.to_string(),
            SettlementData { amount: 42 },
            Some(IDEMPOTENCY.to_string()),
        ))
        .unwrap_err();
        assert_eq!(ret.status().as_u16(), 409);
        assert_eq!(
            ret.body(),
            "Provided idempotency key is tied to other input"
        );

        // fails with different id and different data
        let ret: Response<_> = block_on(alice_engine.send_money(
            "42".to_string(),
            SettlementData { amount: 42 },
            Some(IDEMPOTENCY.to_string()),
        ))
        .unwrap_err();
        assert_eq!(ret.status().as_u16(), 409);
        assert_eq!(
            ret.body(),
            "Provided idempotency key is tied to other input"
        );

        let s = alice_store.clone();
        let cache = s.cache.read();
        let cached_data = cache.get(&IDEMPOTENCY.to_string()).unwrap();

        let cache_hits = s.cache_hits.read();
        assert_eq!(*cache_hits, 4);
        assert_eq!(cached_data.0, 200);
        assert_eq!(cached_data.1, "OK".to_string());

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
            tag: "some_tag".to_string(),
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
        let engine = test_engine(store.clone(), ALICE_PK.clone(), 0, connector_url, false);

        let ret: Response<_> =
            block_on(engine.create_account(bob.id.to_string(), Some(IDEMPOTENCY.to_string())))
                .unwrap();
        assert_eq!(ret.status().as_u16(), 201);
        assert_eq!(ret.body(), "CREATED");

        let ret: Response<_> = engine.get_account(bob.id.to_string()).wait().unwrap();
        assert_eq!(ret.status().as_u16(), 200);

        // check that it's idempotent
        let ret: Response<_> =
            block_on(engine.create_account(bob.id.to_string(), Some(IDEMPOTENCY.to_string())))
                .unwrap();
        assert_eq!(ret.status().as_u16(), 201);
        assert_eq!(ret.body(), "CREATED");

        // fails with different id
        let ret: Response<_> =
            block_on(engine.create_account("42".to_string(), Some(IDEMPOTENCY.to_string())))
                .unwrap_err();
        assert_eq!(ret.status().as_u16(), 409);
        assert_eq!(
            ret.body(),
            "Provided idempotency key is tied to other input"
        );

        // Bob's addresses must be in alice's store now.
        let accs: Vec<Addresses> = store.load_account_addresses(vec![0]).wait().unwrap();
        assert_eq!(accs.len(), 1);
        assert_eq!(accs[0].own_address, bob.address);
        assert_eq!(accs[0].token_address, None);

        let s = store.clone();
        let cache = s.cache.read();
        let cached_data = cache.get(&IDEMPOTENCY.to_string()).unwrap();

        let cache_hits = s.cache_hits.read();
        assert_eq!(*cache_hits, 2);
        assert_eq!(cached_data.0, 201);
        assert_eq!(cached_data.1, "CREATED".to_string());

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
            tag: "some_tag".to_string(),
        })
        .unwrap();

        let m = mockito::mock("POST", MESSAGES_API.clone())
            .with_status(200)
            .with_body(body_se_data)
            .expect(1) // only 1 request is made to the connector (idempotency works properly)
            .create();
        let connector_url = mockito::server_url();

        let store = test_store(ALICE.clone(), false, false, false);
        let engine = test_engine(store.clone(), ALICE_PK.clone(), 0, connector_url, false);

        let ret: Response<_> = block_on(engine.create_account(bob.id.to_string(), None)).unwrap();
        assert_eq!(ret.status().as_u16(), 201);
        assert_eq!(ret.body(), "CREATED");

        let receive_message = ReceiveMessageDetails::new_payment_details_request();
        let receive_message = serde_json::to_vec(&receive_message).unwrap();
        let ret: Response<_> = block_on(engine.receive_message(
            bob.id.to_string(),
            receive_message,
            Some(IDEMPOTENCY.to_owned()),
        ))
        .unwrap();
        assert_eq!(ret.status().as_u16(), 200);

        // the data our SE will return to the node must match alice's addrs
        let alice_addrs = Addresses {
            own_address: ALICE.address,
            token_address: None,
        };
        let data: PaymentDetailsResponse = serde_json::from_str(ret.body()).unwrap();
        assert!(!data.tag.is_empty());
        assert_eq!(data.to, alice_addrs);
        m.assert();
    }

    #[test]
    fn test_save_load_store() {
        let store = test_store(ALICE.clone(), false, false, true);
        store
            .save_account_addresses(
                vec![123],
                vec![Addresses {
                    own_address: BOB.address,
                    token_address: None,
                }],
            )
            .wait()
            .unwrap();

        let id = store
            .load_account_id_from_address(Addresses {
                own_address: BOB.address,
                token_address: None,
            })
            .wait()
            .unwrap();
        assert_eq!(id, 123);
    }
}
