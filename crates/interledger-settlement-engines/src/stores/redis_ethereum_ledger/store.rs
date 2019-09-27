use futures::{
    future::{err, ok},
    Future,
};

use bytes::Bytes;
use http::StatusCode;
use std::collections::HashMap;
use web3::types::{Address as EthAddress, H256, U256};

use crate::engines::ethereum_ledger::{EthereumAccount, EthereumAddresses, EthereumStore};
use num_bigint::BigUint;
use redis::{self, aio::SharedConnection, cmd, ConnectionInfo, PipelineCommands, Value};

use log::{error, trace};

use crate::stores::redis_store_common::{EngineRedisStore, EngineRedisStoreBuilder};
use crate::stores::{IdempotentEngineData, IdempotentEngineStore};
use interledger_settlement::LeftoversStore;

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
    pub(crate) id: String,
    pub(crate) own_address: EthAddress,
    pub(crate) token_address: Option<EthAddress>,
}

fn ethereum_transactions_key(tx_hash: H256) -> String {
    format!(
        "{}:{}:{}:{}",
        ETHEREUM_KEY, LEDGER_KEY, SAVED_TRANSACTIONS_KEY, tx_hash,
    )
}

fn ethereum_ledger_key(account_id: &str) -> String {
    format!(
        "{}:{}:{}:{}",
        ETHEREUM_KEY, LEDGER_KEY, SETTLEMENT_ENGINES_KEY, account_id
    )
}

impl EthereumAccount for Account {
    type AccountId = String;

    fn id(&self) -> Self::AccountId {
        self.id.clone()
    }
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
    pub fn new(redis_url: ConnectionInfo) -> Self {
        EthereumLedgerRedisStoreBuilder {
            redis_store_builder: EngineRedisStoreBuilder::new(redis_url),
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

impl LeftoversStore for EthereumLedgerRedisStore {
    type AccountId = String;
    type AssetType = BigUint;

    fn get_uncredited_settlement_amount(
        &self,
        account_id: Self::AccountId,
    ) -> Box<dyn Future<Item = (Self::AssetType, u8), Error = ()> + Send> {
        self.redis_store
            .get_uncredited_settlement_amount(account_id)
    }

    fn save_uncredited_settlement_amount(
        &self,
        account_id: Self::AccountId,
        uncredited_settlement_amount: (Self::AssetType, u8),
    ) -> Box<dyn Future<Item = (), Error = ()> + Send> {
        self.redis_store
            .save_uncredited_settlement_amount(account_id, uncredited_settlement_amount)
    }

    fn load_uncredited_settlement_amount(
        &self,
        account_id: Self::AccountId,
        local_scale: u8,
    ) -> Box<dyn Future<Item = Self::AssetType, Error = ()> + Send> {
        self.redis_store
            .load_uncredited_settlement_amount(account_id, local_scale)
    }
}

impl IdempotentEngineStore for EthereumLedgerRedisStore {
    fn load_idempotent_data(
        &self,
        idempotency_key: String,
    ) -> Box<dyn Future<Item = Option<IdempotentEngineData>, Error = ()> + Send> {
        self.redis_store.load_idempotent_data(idempotency_key)
    }

    fn save_idempotent_data(
        &self,
        idempotency_key: String,
        input_hash: [u8; 32],
        status_code: StatusCode,
        data: Bytes,
    ) -> Box<dyn Future<Item = (), Error = ()> + Send> {
        self.redis_store
            .save_idempotent_data(idempotency_key, input_hash, status_code, data)
    }
}

impl EthereumStore for EthereumLedgerRedisStore {
    type Account = Account;

    fn load_account_addresses(
        &self,
        account_ids: Vec<String>,
    ) -> Box<dyn Future<Item = Vec<EthereumAddresses>, Error = ()> + Send> {
        let mut pipe = redis::pipe();
        for account_id in account_ids.iter() {
            pipe.hgetall(ethereum_ledger_key(&account_id));
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
                        trace!("Loaded account addresses {:?}", addresses);
                        let mut ret = Vec::with_capacity(addresses.len());
                        for addr in &addresses {
                            let own_address = if let Some(own_address) = addr.get("own_address") {
                                own_address
                            } else {
                                return err(());
                            };
                            let mut out = [0; 20];
                            out.copy_from_slice(own_address);
                            let own_address = EthAddress::from(out);

                            let token_address =
                                if let Some(token_address) = addr.get("token_address") {
                                    token_address
                                } else {
                                    return err(());
                                };
                            let token_address = if token_address.len() == 20 {
                                let mut out = [0; 20];
                                out.copy_from_slice(token_address);
                                Some(EthAddress::from(out))
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
        data: HashMap<String, EthereumAddresses>,
    ) -> Box<dyn Future<Item = (), Error = ()> + Send> {
        let mut pipe = redis::pipe();
        for (account_id, d) in data {
            let token_address = if let Some(token_address) = d.token_address {
                token_address.as_bytes().to_owned()
            } else {
                vec![]
            };
            let acc_id = ethereum_ledger_key(&account_id);
            let addrs = &[
                ("own_address", d.own_address.as_bytes()),
                ("token_address", &token_address),
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
        net_version: String,
        block: U256,
    ) -> Box<dyn Future<Item = (), Error = ()> + Send> {
        let mut pipe = redis::pipe();
        pipe.hset(RECENTLY_OBSERVED_BLOCK_KEY, net_version, block.low_u64())
            .ignore();
        Box::new(
            pipe.query_async(self.connection.clone())
                .map_err(move |err| {
                    error!("Error saving last observed block {:?}: {:?}", block, err)
                })
                .and_then(move |(_conn, _ret): (_, Value)| Ok(())),
        )
    }

    fn load_recently_observed_block(
        &self,
        net_version: String,
    ) -> Box<dyn Future<Item = Option<U256>, Error = ()> + Send> {
        let mut pipe = redis::pipe();
        pipe.get(RECENTLY_OBSERVED_BLOCK_KEY);
        Box::new(
            cmd("HGET")
                .arg(RECENTLY_OBSERVED_BLOCK_KEY)
                .arg(net_version)
                .query_async(self.connection.clone())
                .map_err(move |err| error!("Error loading last observed block: {:?}", err))
                .map(|(_connnection, block): (_, Option<u64>)| block.map(U256::from)),
        )
    }

    fn load_account_id_from_address(
        &self,
        eth_address: EthereumAddresses,
    ) -> Box<dyn Future<Item = String, Error = ()> + Send> {
        Box::new(
            cmd("GET")
                .arg(addrs_to_key(eth_address))
                .query_async(self.connection.clone())
                .map_err(move |err| error!("Error loading account data: {:?}", err))
                .and_then(move |(_conn, account_id): (_, Option<String>)| {
                    if let Some(id) = account_id {
                        Ok(id)
                    } else {
                        error!("Account not found for address: {:?}", eth_address);
                        Err(())
                    }
                }),
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
            let account_ids = vec!["1".to_string(), "2".to_string()];
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
                (account_ids[0].clone(), account_addresses[0]),
                (account_ids[1].clone(), account_addresses[1]),
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
            let block1 = U256::from(1);
            let block2 = U256::from(2);
            store
                .save_recently_observed_block("1".to_owned(), block1)
                .map_err(|err| eprintln!("Redis error: {:?}", err))
                .join(store.save_recently_observed_block("2".to_owned(), block2))
                .map_err(|err| eprintln!("Redis error: {:?}", err))
                .and_then(move |_| {
                    store
                        .load_recently_observed_block("1".to_owned())
                        .map_err(|err| eprintln!("Redis error: {:?}", err))
                        .join(
                            store
                                .load_recently_observed_block("2".to_owned())
                                .map_err(|err| eprintln!("Redis error: {:?}", err)),
                        )
                        .and_then(move |(ret1, ret2)| {
                            assert_eq!(ret1, Some(block1));
                            assert_eq!(ret2, Some(block2));
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
                H256::from_str("b28675771f555adf614f1401838b9fffb43bc285387679bcbd313a8dc5bdc00e")
                    .unwrap();
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
