use bytes::Bytes;
use interledger_service::{Account, AccountStore};
use tokio::runtime::Runtime;

use parking_lot::RwLock;
use std::collections::HashMap;
use std::sync::Arc;

use hyper::StatusCode;
use num_traits::Zero;
use std::process::Command;
use std::str::FromStr;
use std::thread::sleep;
use std::time::Duration;

use web3::{
    futures::future::{err, ok, Future},
    types::{Address, H256, U256},
};

use super::eth_engine::{EthereumLedgerSettlementEngine, EthereumLedgerSettlementEngineBuilder};
use super::types::{Addresses, EthereumAccount, EthereumLedgerTxSigner, EthereumStore};
use interledger_settlement::{IdempotentData, IdempotentStore};

#[derive(Debug, Clone)]
pub struct TestAccount {
    pub id: u64,
    pub address: Address,
    pub token_address: Address,
    pub no_details: bool,
}

impl Account for TestAccount {
    type AccountId = u64;

    fn id(&self) -> u64 {
        self.id
    }
}

impl EthereumAccount for TestAccount {
    fn token_address(&self) -> Option<Address> {
        if self.no_details {
            return None;
        }
        Some(self.token_address)
    }
    fn own_address(&self) -> Address {
        self.address
    }
}

// Test Store
#[derive(Clone)]
pub struct TestStore {
    pub accounts: Arc<Vec<TestAccount>>,
    pub should_fail: bool,
    pub addresses: Arc<RwLock<HashMap<u64, Addresses>>>,
    pub address_to_id: Arc<RwLock<HashMap<Addresses, u64>>>,
    #[allow(clippy::all)]
    pub cache: Arc<RwLock<HashMap<String, (StatusCode, String, [u8; 32])>>>,
    pub last_observed_block: Arc<RwLock<U256>>,
    pub saved_hashes: Arc<RwLock<HashMap<H256, bool>>>,
    pub cache_hits: Arc<RwLock<u64>>,
    pub uncredited_settlement_amount: Arc<RwLock<HashMap<String, BigUint>>>,
}

use crate::stores::LeftoversStore;
use num_bigint::BigUint;

impl LeftoversStore for TestStore {
    type AssetType = BigUint;

    fn save_uncredited_settlement_amount(
        &self,
        account_id: String,
        uncredited_settlement_amount: Self::AssetType,
    ) -> Box<dyn Future<Item = (), Error = ()> + Send> {
        let mut guard = self.uncredited_settlement_amount.write();
        (*guard).insert(account_id, uncredited_settlement_amount);
        Box::new(ok(()))
    }

    fn load_uncredited_settlement_amount(
        &self,
        account_id: String,
    ) -> Box<dyn Future<Item = Self::AssetType, Error = ()> + Send> {
        let mut guard = self.uncredited_settlement_amount.write();
        if let Some(l) = guard.get(&account_id) {
            let l = l.clone();
            (*guard).insert(account_id, Zero::zero());
            Box::new(ok(l.clone()))
        } else {
            Box::new(ok(Zero::zero()))
        }
    }
}

impl EthereumStore for TestStore {
    type Account = TestAccount;

    fn save_account_addresses(
        &self,
        data: HashMap<u64, Addresses>,
    ) -> Box<dyn Future<Item = (), Error = ()> + Send> {
        let mut guard = self.addresses.write();
        let mut guard2 = self.address_to_id.write();
        for (acc, d) in data {
            (*guard).insert(acc, d);
            (*guard2).insert(d, acc);
        }
        Box::new(ok(()))
    }

    fn load_account_addresses(
        &self,
        account_ids: Vec<u64>,
    ) -> Box<dyn Future<Item = Vec<Addresses>, Error = ()> + Send> {
        let mut v = Vec::with_capacity(account_ids.len());
        let addresses = self.addresses.read();
        for acc in &account_ids {
            if let Some(d) = addresses.get(&acc) {
                v.push(Addresses {
                    own_address: d.own_address,
                    token_address: d.token_address,
                });
            } else {
                // if the account is not found, error out
                return Box::new(err(()));
            }
        }
        Box::new(ok(v))
    }

    fn save_recently_observed_block(
        &self,
        block: U256,
    ) -> Box<dyn Future<Item = (), Error = ()> + Send> {
        let mut guard = self.last_observed_block.write();
        *guard = block;
        Box::new(ok(()))
    }

    fn load_recently_observed_block(
        &self,
    ) -> Box<dyn Future<Item = Option<U256>, Error = ()> + Send> {
        Box::new(Some(ok(*self.last_observed_block.read())))
    }

    fn load_account_id_from_address(
        &self,
        eth_address: Addresses,
    ) -> Box<dyn Future<Item = u64, Error = ()> + Send> {
        let addresses = self.address_to_id.read();
        let d = if let Some(d) = addresses.get(&eth_address) {
            *d
        } else {
            return Box::new(err(()));
        };

        Box::new(ok(d))
    }

    fn check_if_tx_processed(
        &self,
        tx_hash: H256,
    ) -> Box<dyn Future<Item = bool, Error = ()> + Send> {
        let hashes = self.saved_hashes.read();
        // if hash exists then return error
        if hashes.get(&tx_hash).is_some() {
            Box::new(ok(true))
        } else {
            Box::new(ok(false))
        }
    }

    fn mark_tx_processed(&self, tx_hash: H256) -> Box<dyn Future<Item = (), Error = ()> + Send> {
        let mut hashes = self.saved_hashes.write();
        (*hashes).insert(tx_hash, true);
        Box::new(ok(()))
    }
}

impl AccountStore for TestStore {
    type Account = TestAccount;

    fn get_accounts(
        &self,
        account_ids: Vec<<<Self as AccountStore>::Account as Account>::AccountId>,
    ) -> Box<Future<Item = Vec<Self::Account>, Error = ()> + Send> {
        let accounts: Vec<TestAccount> = self
            .accounts
            .iter()
            .filter_map(|account| {
                if account_ids.contains(&account.id) {
                    Some(account.clone())
                } else {
                    None
                }
            })
            .collect();
        if accounts.len() == account_ids.len() {
            Box::new(ok(accounts))
        } else {
            Box::new(err(()))
        }
    }
}

impl IdempotentStore for TestStore {
    fn load_idempotent_data(
        &self,
        idempotency_key: String,
    ) -> Box<dyn Future<Item = Option<IdempotentData>, Error = ()> + Send> {
        let cache = self.cache.read();
        if let Some(data) = cache.get(&idempotency_key) {
            let mut guard = self.cache_hits.write();
            *guard += 1; // used to test how many times this branch gets executed
            Box::new(ok(Some((data.0, Bytes::from(data.1.clone()), data.2))))
        } else {
            Box::new(ok(None))
        }
    }

    fn save_idempotent_data(
        &self,
        idempotency_key: String,
        input_hash: [u8; 32],
        status_code: StatusCode,
        data: Bytes,
    ) -> Box<dyn Future<Item = (), Error = ()> + Send> {
        let mut cache = self.cache.write();
        cache.insert(
            idempotency_key,
            (
                status_code,
                String::from_utf8_lossy(&data).to_string(),
                input_hash,
            ),
        );
        Box::new(ok(()))
    }
}

impl TestStore {
    pub fn new(accs: Vec<TestAccount>, should_fail: bool, initialize: bool) -> Self {
        let mut addresses = HashMap::new();
        let mut address_to_id = HashMap::new();
        if initialize {
            for account in &accs {
                let token_address = if !account.no_details {
                    Some(account.token_address)
                } else {
                    None
                };
                let addrs = Addresses {
                    own_address: account.address,
                    token_address,
                };
                addresses.insert(account.id, addrs);
                address_to_id.insert(addrs, account.id);
            }
        }

        TestStore {
            accounts: Arc::new(accs),
            should_fail,
            addresses: Arc::new(RwLock::new(addresses)),
            address_to_id: Arc::new(RwLock::new(address_to_id)),
            cache: Arc::new(RwLock::new(HashMap::new())),
            cache_hits: Arc::new(RwLock::new(0)),
            last_observed_block: Arc::new(RwLock::new(U256::from(0))),
            saved_hashes: Arc::new(RwLock::new(HashMap::new())),
            uncredited_settlement_amount: Arc::new(RwLock::new(HashMap::new())),
        }
    }
}

// Test Service

impl TestAccount {
    pub fn new(id: u64, address: &str, token_address: &str) -> Self {
        Self {
            id,
            address: Address::from_str(address).unwrap(),
            token_address: Address::from_str(token_address).unwrap(),
            no_details: false,
        }
    }
}

// Helper to create a new engine and spin a new ganache instance.
pub fn test_engine<Si, S, A>(
    store: S,
    key: Si,
    confs: u8,
    connector_url: &str,
    watch_incoming: bool,
) -> EthereumLedgerSettlementEngine<S, Si, A>
where
    Si: EthereumLedgerTxSigner + Clone + Send + Sync + 'static,
    S: EthereumStore<Account = A>
        + LeftoversStore<AssetType = BigUint>
        + IdempotentStore
        + Clone
        + Send
        + Sync
        + 'static,
    A: EthereumAccount + Send + Sync + 'static,
{
    EthereumLedgerSettlementEngineBuilder::new(store, key)
        .connector_url(connector_url)
        .confirmations(confs)
        .watch_incoming(watch_incoming)
        .poll_frequency(1000)
        .connect()
}

pub fn start_ganache() -> std::process::Child {
    let mut ganache = Command::new("ganache-cli");
    let ganache = ganache.stdout(std::process::Stdio::null()).arg("-m").arg(
        "abstract vacuum mammal awkward pudding scene penalty purchase dinner depart evoke puzzle",
    );
    let ganache_pid = ganache.spawn().expect("couldnt start ganache-cli");
    // wait a couple of seconds for ganache to boot up
    sleep(Duration::from_secs(5));
    ganache_pid
}

pub fn test_store(
    account: TestAccount,
    store_fails: bool,
    account_has_engine: bool,
    initialize: bool,
) -> TestStore {
    let mut acc = account.clone();
    acc.no_details = !account_has_engine;
    TestStore::new(vec![acc], store_fails, initialize)
}

// Futures helper taken from the store_helpers in interledger-store-redis.
pub fn block_on<F>(f: F) -> Result<F::Item, F::Error>
where
    F: Future + Send + 'static,
    F::Item: Send,
    F::Error: Send,
{
    let _ = env_logger::try_init();
    let mut runtime = Runtime::new().unwrap();
    runtime.block_on(f)
}
