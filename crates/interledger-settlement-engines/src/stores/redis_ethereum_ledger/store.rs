use futures::{
    future::{err, ok, result},
    Future,
};
use std::collections::HashMap as SlowHashMap;

use ethereum_tx_sign::web3::types::{Address as EthAddress, H256, U256};
use interledger_service::Account as AccountTrait;

use crate::{EthereumAccount, EthereumAddresses, EthereumStore};
use bytes::Bytes;
use http::StatusCode;
use interledger_settlement::{IdempotentData, IdempotentStore};
use redis::{
    self, cmd, r#async::SharedConnection, Client, ConnectionInfo, PipelineCommands, Value,
};
use std::str::FromStr;
use std::{str, sync::Arc};

use log::{debug, error, trace};

static RECENTLY_OBSERVED_DATA_KEY: &str = "recently_observed_data";
static SETTLEMENT_ENGINES_KEY: &str = "settlement";
static LEDGER_KEY: &str = "ledger";
static ETHEREUM_KEY: &str = "eth";

fn ethereum_ledger_key(account_id: u64) -> String {
    format!(
        "{}:{}:{}:{}",
        SETTLEMENT_ENGINES_KEY, LEDGER_KEY, ETHEREUM_KEY, account_id
    )
}

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

impl EthereumAccount for Account {
    fn token_address(&self) -> Option<EthAddress> {
        self.token_address
    }

    fn own_address(&self) -> EthAddress {
        self.own_address
    }
}

pub struct EthereumLedgerRedisStoreBuilder {
    redis_uri: ConnectionInfo,
}

impl EthereumLedgerRedisStoreBuilder {
    pub fn new(redis_uri: ConnectionInfo) -> Self {
        EthereumLedgerRedisStoreBuilder { redis_uri }
    }

    pub fn connect(&self) -> impl Future<Item = EthereumLedgerRedisStore, Error = ()> {
        result(Client::open(self.redis_uri.clone()))
            .map_err(|err| error!("Error creating Redis client: {:?}", err))
            .and_then(|client| {
                debug!("Connected to redis: {:?}", client);
                client
                    .get_shared_async_connection()
                    .map_err(|err| error!("Error connecting to Redis: {:?}", err))
            })
            .and_then(move |connection| {
                let store = EthereumLedgerRedisStore {
                    connection: Arc::new(connection),
                };
                Ok(store)
            })
    }
}

/// An Ethereum Store that uses Redis as its underlying database.
///
/// This store saves all Ethereum Ledger data for the Ethereum Settlement engine
#[derive(Clone)]
pub struct EthereumLedgerRedisStore {
    connection: Arc<SharedConnection>,
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
            pipe.query_async(self.connection.as_ref().clone())
                .map_err(move |err| {
                    error!(
                        "Error the addresses for accounts: {:?} {:?}",
                        account_ids, err
                    )
                })
                .and_then(
                    move |(_conn, addresses): (_, Vec<SlowHashMap<String, Vec<u8>>>)| {
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
        account_ids: Vec<<Self::Account as AccountTrait>::AccountId>,
        data: Vec<EthereumAddresses>,
    ) -> Box<dyn Future<Item = (), Error = ()> + Send> {
        let mut pipe = redis::pipe();
        for (account_id, d) in account_ids.iter().zip(&data) {
            let token_address = if let Some(token_address) = d.token_address {
                token_address.to_vec()
            } else {
                vec![]
            };
            let acc_id = ethereum_ledger_key(*account_id);
            let addrs = &[
                ("own_address", d.own_address.to_vec()),
                ("token_address", token_address),
            ];
            pipe.hset_multiple(acc_id, addrs).ignore();
            pipe.set(addrs_to_key(*d), *account_id).ignore();
        }
        Box::new(
            pipe.query_async(self.connection.as_ref().clone())
                .map_err(move |err| {
                    error!(
                        "Error saving account data for accounts: {:?} {:?}",
                        account_ids, err
                    )
                })
                .and_then(move |(_conn, _ret): (_, Value)| Ok(())),
        )
    }

    fn save_recently_observed_data(
        &self,
        block: U256,
        balance: U256,
    ) -> Box<dyn Future<Item = (), Error = ()> + Send> {
        let mut pipe = redis::pipe();
        let value = &[("block", block.low_u64()), ("balance", balance.low_u64())];
        pipe.hset_multiple(RECENTLY_OBSERVED_DATA_KEY, value)
            .ignore();
        Box::new(
            pipe.query_async(self.connection.as_ref().clone())
                .map_err(move |err| {
                    error!(
                        "Error saving last observed block {:?} and balance {:?}: {:?}",
                        block, balance, err
                    )
                })
                .and_then(move |(_conn, _ret): (_, Value)| Ok(())),
        )
    }

    fn load_recently_observed_data(
        &self,
    ) -> Box<dyn Future<Item = (U256, U256), Error = ()> + Send> {
        let mut pipe = redis::pipe();
        pipe.hgetall(RECENTLY_OBSERVED_DATA_KEY);
        Box::new(
            pipe.query_async(self.connection.as_ref().clone())
                .map_err(move |err| error!("Error loading last observed block: {:?}", err))
                .and_then(move |(_conn, data): (_, Vec<SlowHashMap<String, u64>>)| {
                    let data = &data[0];
                    let block = if let Some(block) = data.get("block") {
                        block
                    } else {
                        &0 // return 0 if not found
                    };
                    let block = U256::from(*block);

                    let balance = if let Some(balance) = data.get("balance") {
                        balance
                    } else {
                        &0
                    };
                    let balance = U256::from(*balance);
                    ok((block, balance))
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
            pipe.query_async(self.connection.as_ref().clone())
                .map_err(move |err| error!("Error loading account data: {:?}", err))
                .and_then(move |(_conn, account_id): (_, Vec<u64>)| ok(account_id[0])),
        )
    }

    fn check_tx_credited(&self, tx_hash: H256) -> Box<dyn Future<Item = (), Error = ()> + Send> {
        Box::new(
            cmd("SETNX")
                .arg(tx_hash.to_string())
                .arg(true)
                .query_async(self.connection.as_ref().clone())
                .map_err(move |err| error!("Error loading account data: {:?}", err))
                .and_then(
                    move |(_conn, ret): (_, u64)| {
                        if ret == 1 {
                            ok(())
                        } else {
                            err(())
                        }
                    },
                ),
        )
    }
}

impl IdempotentStore for EthereumLedgerRedisStore {
    fn load_idempotent_data(
        &self,
        idempotency_key: String,
    ) -> Box<dyn Future<Item = Option<IdempotentData>, Error = ()> + Send> {
        let idempotency_key_clone = idempotency_key.clone();
        Box::new(
            cmd("HGETALL")
                .arg(idempotency_key.clone())
                .query_async(self.connection.as_ref().clone())
                .map_err(move |err| {
                    error!(
                        "Error loading idempotency key {}: {:?}",
                        idempotency_key_clone, err
                    )
                })
                .and_then(move |(_connection, ret): (_, SlowHashMap<String, String>)| {
                    let data = if let (Some(status_code), Some(data), Some(input_hash_slice)) = (
                        ret.get("status_code"),
                        ret.get("data"),
                        ret.get("input_hash"),
                    ) {
                        trace!("Loaded idempotency key {:?} - {:?}", idempotency_key, ret);
                        let mut input_hash: [u8; 32] = Default::default();
                        input_hash.copy_from_slice(input_hash_slice.as_ref());
                        Some((
                            StatusCode::from_str(status_code).unwrap(),
                            Bytes::from(data.clone()),
                            input_hash,
                        ))
                    } else {
                        None
                    };
                    Ok(data)
                }),
        )
    }

    fn save_idempotent_data(
        &self,
        idempotency_key: String,
        input_hash: [u8; 32],
        status_code: StatusCode,
        data: Bytes,
    ) -> Box<dyn Future<Item = (), Error = ()> + Send> {
        let mut pipe = redis::pipe();
        pipe.atomic()
            .cmd("HMSET") // cannot use hset_multiple since data and status_code have different types
            .arg(&idempotency_key)
            .arg("status_code")
            .arg(status_code.as_u16())
            .arg("data")
            .arg(data.as_ref())
            .arg("input_hash")
            .arg(&input_hash)
            .ignore()
            .expire(&idempotency_key, 86400)
            .ignore();
        Box::new(
            pipe.query_async(self.connection.as_ref().clone())
                .map_err(|err| error!("Error caching: {:?}", err))
                .and_then(move |(_connection, _): (_, Vec<String>)| {
                    trace!(
                        "Cached {:?}: {:?}, {:?}",
                        idempotency_key,
                        status_code,
                        data,
                    );
                    Ok(())
                }),
        )
    }
}

fn addrs_to_key(address: EthereumAddresses) -> String {
    let token_address = if let Some(token_address) = address.token_address {
        token_address.to_string()
    } else {
        "".to_string()
    };
    format!("{}:{}", address.own_address.to_string(), token_address)
}

#[cfg(test)]
mod tests {
    use super::super::store_helpers::*;
    use super::*;
    use futures::future;
    use redis::IntoConnectionInfo;
    use tokio::runtime::Runtime;

    #[test]
    fn connect_fails_if_db_unavailable() {
        let mut runtime = Runtime::new().unwrap();
        runtime
            .block_on(future::lazy(
                || -> Box<dyn Future<Item = (), Error = ()> + Send> {
                    Box::new(
                        EthereumLedgerRedisStoreBuilder::new(
                            "redis://127.0.0.1:0".into_connection_info().unwrap() as ConnectionInfo,
                        )
                        .connect()
                        .then(|result| {
                            assert!(result.is_err());
                            Ok(())
                        }),
                    )
                },
            ))
            .unwrap();
    }

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
            store
                .save_account_addresses(account_ids.clone(), account_addresses.clone())
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
            let balance = U256::from(123);
            store
                .save_recently_observed_data(block, balance)
                .map_err(|err| eprintln!("Redis error: {:?}", err))
                .and_then(move |_| {
                    store
                        .load_recently_observed_data()
                        .map_err(|err| eprintln!("Redis error: {:?}", err))
                        .and_then(move |data| {
                            assert_eq!(data.0, block);
                            assert_eq!(data.1, balance);
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
                .check_tx_credited(tx_hash)
                .map_err(|err| eprintln!("Redis error: {:?}", err))
                .and_then(move |_| {
                    store
                        .check_tx_credited(tx_hash)
                        .map_err(|err| eprintln!("Redis error: {:?}", err))
                        .and_then(move |_| {
                            let _ = context;
                            Ok(())
                        })
                })
        }))
        .unwrap_err()
    }
}
