use futures::{
    future::{err, ok},
    Future,
};

use ethereum_tx_sign::web3::types::{Address as EthAddress, H256, U256};
use interledger_service::Account as AccountTrait;
use std::collections::HashMap;

use crate::engines::ethereum_ledger::{EthereumAccount, EthereumAddresses, EthereumStore};
use redis::{self, cmd, r#async::SharedConnection, ConnectionInfo, PipelineCommands, Value};

use log::{debug, error};

use crate::stores::redis_store::{EngineRedisStore, EngineRedisStoreBuilder};

// Key for the latest observed block and balance. The data is stored in order to
// avoid double crediting transactions which have already been processed, and in
// order to resume watching from the last observed point.
static RECENTLY_OBSERVED_BLOCK_KEY: &str = "recently_observed_block";
static SAVED_TRANSACTIONS_KEY: &str = "transactions";
static SETTLEMENT_ENGINES_KEY: &str = "settlement";
static LEDGER_KEY: &str = "ledger";
static ETHEREUM_KEY: &str = "eth";

#[derive(Clone, Debug, Serialize)]
pub struct Account {
    pub(crate) id: u64,
    pub(crate) own_address: EthAddress,
    pub(crate) token_address: Option<EthAddress>,
}

impl AccountTrait for Account {
    type AccountId = u64;

    fn id(&self) -> Self::AccountId {
        self.id
    }
}

fn ethereum_transactions_key(tx_hash: H256) -> String {
    format!(
        "{}:{}:{}:{}",
        ETHEREUM_KEY, LEDGER_KEY, SAVED_TRANSACTIONS_KEY, tx_hash,
    )
}

fn ethereum_ledger_key(account_id: u64) -> String {
    format!(
        "{}:{}:{}:{}",
        ETHEREUM_KEY, LEDGER_KEY, SETTLEMENT_ENGINES_KEY, account_id
    )
}

impl EthereumAccount for Account {
    fn token_address(&self) -> Option<EthAddress> {
        self.token_address
    }

    fn own_address(&self) -> EthAddress {
        self.own_address
    }
}

pub struct EthereumLedgerRedisStoreBuilder {
    redis_store_builder: EngineRedisStoreBuilder,
}

impl EthereumLedgerRedisStoreBuilder {
    pub fn new(redis_uri: ConnectionInfo) -> Self {
        EthereumLedgerRedisStoreBuilder {
            redis_store_builder: EngineRedisStoreBuilder::new(redis_uri),
        }
    }

    pub fn connect(&self) -> impl Future<Item = EthereumLedgerRedisStore, Error = ()> {
        self.redis_store_builder
            .connect()
            .and_then(move |redis_store| {
                let connection = redis_store.connection.clone();
                Ok(EthereumLedgerRedisStore {
                    redis_store,
                    connection,
                })
            })
    }
}

/// An Ethereum Store that uses Redis as its underlying database.
///
/// This store saves all Ethereum Ledger data for the Ethereum Settlement engine
#[derive(Clone)]
pub struct EthereumLedgerRedisStore {
    redis_store: EngineRedisStore,
    connection: SharedConnection,
}

impl EthereumLedgerRedisStore {
    pub fn new(redis_store: EngineRedisStore) -> Self {
        let connection = redis_store.connection.clone();
        EthereumLedgerRedisStore {
            redis_store,
            connection,
        }
    }
}

impl EthereumStore for EthereumLedgerRedisStore {
    type Account = Account;

    fn load_account_addresses(
        &self,
        account_ids: Vec<<Self::Account as AccountTrait>::AccountId>,
    ) -> Box<dyn Future<Item = Vec<EthereumAddresses>, Error = ()> + Send> {
        debug!("Loading account addresses {:?}", account_ids);
        let mut pipe = redis::pipe();
        for account_id in account_ids.iter() {
            pipe.hgetall(ethereum_ledger_key(*account_id));
        }
        Box::new(
            pipe.query_async(self.connection.clone())
                .map_err(move |err| {
                    error!(
                        "Error the addresses for accounts: {:?} {:?}",
                        account_ids, err
                    )
                })
                .and_then(
                    move |(_conn, addresses): (_, Vec<HashMap<String, Vec<u8>>>)| {
                        debug!("Loaded account addresses {:?}", addresses);
                        let mut ret = Vec::with_capacity(addresses.len());
                        for addr in &addresses {
                            let own_address = if let Some(own_address) = addr.get("own_address") {
                                own_address
                            } else {
                                return err(());
                            };
                            let own_address = EthAddress::from(&own_address[..]);

                            let token_address =
                                if let Some(token_address) = addr.get("token_address") {
                                    token_address
                                } else {
                                    return err(());
                                };
                            let token_address = if token_address.len() == 20 {
                                Some(EthAddress::from(&token_address[..]))
                            } else {
                                None
                            };
                            ret.push(EthereumAddresses {
                                own_address,
                                token_address,
                            });
                        }
                        ok(ret)
                    },
                ),
        )
    }

    fn save_account_addresses(
        &self,
        data: HashMap<<Self::Account as AccountTrait>::AccountId, EthereumAddresses>,
    ) -> Box<dyn Future<Item = (), Error = ()> + Send> {
        let mut pipe = redis::pipe();
        for (account_id, d) in data {
            let token_address = if let Some(token_address) = d.token_address {
                token_address.to_vec()
            } else {
                vec![]
            };
            let acc_id = ethereum_ledger_key(account_id);
            let addrs = &[
                ("own_address", d.own_address.to_vec()),
                ("token_address", token_address),
            ];
            pipe.hset_multiple(acc_id, addrs).ignore();
            pipe.set(addrs_to_key(d), account_id).ignore();
        }
        Box::new(
            pipe.query_async(self.connection.clone())
                .map_err(move |err| error!("Error saving account data: {:?}", err))
                .and_then(move |(_conn, _ret): (_, Value)| Ok(())),
        )
    }

    fn save_recently_observed_block(
        &self,
        block: U256,
    ) -> Box<dyn Future<Item = (), Error = ()> + Send> {
        let mut pipe = redis::pipe();
        pipe.set(RECENTLY_OBSERVED_BLOCK_KEY, block.low_u64())
            .ignore();
        Box::new(
            pipe.query_async(self.connection.clone())
                .map_err(move |err| {
                    error!("Error saving last observed block {:?}: {:?}", block, err)
                })
                .and_then(move |(_conn, _ret): (_, Value)| Ok(())),
        )
    }

    fn load_recently_observed_block(&self) -> Box<dyn Future<Item = U256, Error = ()> + Send> {
        let mut pipe = redis::pipe();
        pipe.get(RECENTLY_OBSERVED_BLOCK_KEY);
        Box::new(
            pipe.query_async(self.connection.clone())
                .map_err(move |err| error!("Error loading last observed block: {:?}", err))
                .and_then(move |(_conn, block): (_, Vec<u64>)| {
                    if !block.is_empty() {
                        let block = U256::from(block[0]);
                        ok(block)
                    } else {
                        ok(U256::from(0))
                    }
                }),
        )
    }

    fn load_account_id_from_address(
        &self,
        eth_address: EthereumAddresses,
    ) -> Box<dyn Future<Item = <Self::Account as AccountTrait>::AccountId, Error = ()> + Send> {
        let mut pipe = redis::pipe();
        pipe.get(addrs_to_key(eth_address));
        Box::new(
            pipe.query_async(self.connection.clone())
                .map_err(move |err| error!("Error loading account data: {:?}", err))
                .and_then(move |(_conn, account_id): (_, Vec<u64>)| ok(account_id[0])),
        )
    }

    fn check_if_tx_processed(
        &self,
        tx_hash: H256,
    ) -> Box<dyn Future<Item = bool, Error = ()> + Send> {
        Box::new(
            cmd("EXISTS")
                .arg(ethereum_transactions_key(tx_hash))
                .query_async(self.connection.clone())
                .map_err(move |err| error!("Error loading account data: {:?}", err))
                .and_then(move |(_conn, ret): (_, bool)| Ok(ret)),
        )
    }

    fn mark_tx_processed(&self, tx_hash: H256) -> Box<dyn Future<Item = (), Error = ()> + Send> {
        Box::new(
            cmd("SETNX")
                .arg(ethereum_transactions_key(tx_hash))
                .arg(true)
                .query_async(self.connection.clone())
                .map_err(move |err| error!("Error loading account data: {:?}", err))
                .and_then(
                    move |(_conn, ret): (_, bool)| {
                        if ret {
                            ok(())
                        } else {
                            err(())
                        }
                    },
                ),
        )
    }
}

fn addrs_to_key(address: EthereumAddresses) -> String {
    let token_address = if let Some(token_address) = address.token_address {
        token_address.to_string()
    } else {
        "null".to_string()
    };
    format!(
        "account:{}:{}",
        address.own_address.to_string(),
        token_address
    )
}

#[cfg(test)]
mod tests {
    use super::super::super::test_helpers::store_helpers::{
        block_on, test_eth_store as test_store,
    };
    use super::*;
    use std::iter::FromIterator;
    use std::str::FromStr;

    #[test]
    fn saves_and_loads_ethereum_addreses_properly() {
        block_on(test_store().and_then(|(store, context)| {
            let account_ids = vec![30, 42];
            let account_addresses = vec![
                EthereumAddresses {
                    own_address: EthAddress::from_str("3cdb3d9e1b74692bb1e3bb5fc81938151ca64b02")
                        .unwrap(),
                    token_address: Some(
                        EthAddress::from_str("c92be489639a9c61f517bd3b955840fa19bc9b7c").unwrap(),
                    ),
                },
                EthereumAddresses {
                    own_address: EthAddress::from_str("2fcd07047c209c46a767f8338cb0b14955826826")
                        .unwrap(),
                    token_address: None,
                },
            ];
            let input = HashMap::from_iter(vec![
                (account_ids[0], account_addresses[0]),
                (account_ids[1], account_addresses[1]),
            ]);
            store
                .save_account_addresses(input)
                .map_err(|err| eprintln!("Redis error: {:?}", err))
                .and_then(move |_| {
                    store
                        .load_account_addresses(account_ids.clone())
                        .map_err(|err| eprintln!("Redis error: {:?}", err))
                        .and_then(move |data| {
                            assert_eq!(data[0], account_addresses[0]);
                            assert_eq!(data[1], account_addresses[1]);
                            let _ = context;
                            store
                                .load_account_id_from_address(account_addresses[0])
                                .map_err(|err| eprintln!("Redis error: {:?}", err))
                                .and_then(move |acc_id| {
                                    assert_eq!(acc_id, account_ids[0]);
                                    let _ = context;
                                    store
                                        .load_account_id_from_address(account_addresses[1])
                                        .map_err(|err| eprintln!("Redis error: {:?}", err))
                                        .and_then(move |acc_id| {
                                            assert_eq!(acc_id, account_ids[1]);
                                            let _ = context;
                                            Ok(())
                                        })
                                })
                        })
                })
        }))
        .unwrap()
    }

    #[test]
    fn saves_and_loads_last_observed_data_properly() {
        block_on(test_store().and_then(|(store, context)| {
            let block = U256::from(2);
            store
                .save_recently_observed_block(block)
                .map_err(|err| eprintln!("Redis error: {:?}", err))
                .and_then(move |_| {
                    store
                        .load_recently_observed_block()
                        .map_err(|err| eprintln!("Redis error: {:?}", err))
                        .and_then(move |data| {
                            assert_eq!(data, block);
                            let _ = context;
                            Ok(())
                        })
                })
        }))
        .unwrap()
    }

    #[test]
    fn saves_tx_hashes_properly() {
        block_on(test_store().and_then(|(store, context)| {
            let tx_hash =
                H256::from("0xb28675771f555adf614f1401838b9fffb43bc285387679bcbd313a8dc5bdc00e");
            store
                .mark_tx_processed(tx_hash)
                .map_err(|err| eprintln!("Redis error: {:?}", err))
                .and_then(move |_| {
                    store
                        .check_if_tx_processed(tx_hash)
                        .map_err(|err| eprintln!("Redis error: {:?}", err))
                        .and_then(move |seen2| {
                            assert_eq!(seen2, true);
                            let _ = context;
                            Ok(())
                        })
                })
        }))
        .unwrap()
    }
}
