use super::*;
use crate::api::scale_with_precision_loss;
use crate::Convert;
use crate::{LeftoversStore, SettlementEngineDetails};
use futures::{
    future::{err, ok},
    Future,
};
use interledger_service::{
    incoming_service_fn, outgoing_service_fn, Account, AccountStore, IncomingService,
    OutgoingService, Username,
};

use interledger_packet::{Address, ErrorCode, FulfillBuilder, RejectBuilder};
use mockito::mock;

use crate::fixtures::{BODY, MESSAGES_API, SERVICE_ADDRESS, SETTLEMENT_API, TEST_ACCOUNT_0};
use lazy_static::lazy_static;
use parking_lot::RwLock;
use std::collections::HashMap;
use std::str::FromStr;
use std::sync::Arc;
use tokio::runtime::Runtime;
use url::Url;

#[derive(Debug, Clone)]
pub struct TestAccount {
    pub id: u64,
    pub url: Url,
    pub ilp_address: Address,
    pub no_details: bool,
    pub balance: i64,
}

lazy_static! {
    pub static ref ALICE: Username = Username::from_str("alice").unwrap();
}

impl Account for TestAccount {
    type AccountId = u64;

    fn id(&self) -> u64 {
        self.id
    }

    fn username(&self) -> &Username {
        &ALICE
    }

    fn asset_code(&self) -> &str {
        "XYZ"
    }

    // All connector accounts use asset scale = 9.
    fn asset_scale(&self) -> u8 {
        9
    }

    fn ilp_address(&self) -> &Address {
        &self.ilp_address
    }
}
impl SettlementAccount for TestAccount {
    fn settlement_engine_details(&self) -> Option<SettlementEngineDetails> {
        if self.no_details {
            return None;
        }
        Some(SettlementEngineDetails {
            url: self.url.clone(),
        })
    }
}

// Test Store
#[derive(Clone)]
pub struct TestStore {
    pub accounts: Arc<RwLock<Vec<TestAccount>>>,
    pub should_fail: bool,
    pub cache: Arc<RwLock<HashMap<String, IdempotentData>>>,
    pub cache_hits: Arc<RwLock<u64>>,
    pub uncredited_settlement_amount: Arc<RwLock<HashMap<u64, (BigUint, u8)>>>,
}

impl SettlementStore for TestStore {
    type Account = TestAccount;

    fn update_balance_for_incoming_settlement(
        &self,
        account_id: <Self::Account as Account>::AccountId,
        amount: u64,
        _idempotency_key: Option<String>,
    ) -> Box<dyn Future<Item = (), Error = ()> + Send> {
        let mut accounts = self.accounts.write();
        for mut a in &mut *accounts {
            if a.id() == account_id {
                a.balance += amount as i64;
            }
        }
        let ret = if self.should_fail { err(()) } else { ok(()) };
        Box::new(ret)
    }

    fn refund_settlement(
        &self,
        _account_id: <Self::Account as Account>::AccountId,
        _settle_amount: u64,
    ) -> Box<dyn Future<Item = (), Error = ()> + Send> {
        let ret = if self.should_fail { err(()) } else { ok(()) };
        Box::new(ret)
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
            Box::new(ok(Some((data.0, data.1.clone(), data.2))))
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
        cache.insert(idempotency_key, (status_code, data, input_hash));
        Box::new(ok(()))
    }
}

impl AccountStore for TestStore {
    type Account = TestAccount;

    fn get_accounts(
        &self,
        account_ids: Vec<<<Self as AccountStore>::Account as Account>::AccountId>,
    ) -> Box<dyn Future<Item = Vec<Self::Account>, Error = ()> + Send> {
        let accounts: Vec<TestAccount> = self
            .accounts
            .read()
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

    // stub implementation (not used in these tests)
    fn get_account_id_from_username(
        &self,
        _username: &Username,
    ) -> Box<dyn Future<Item = u64, Error = ()> + Send> {
        Box::new(ok(1))
    }
}

impl LeftoversStore for TestStore {
    type AccountId = u64;
    type AssetType = BigUint;

    fn save_uncredited_settlement_amount(
        &self,
        account_id: Self::AccountId,
        uncredited_settlement_amount: (Self::AssetType, u8),
    ) -> Box<dyn Future<Item = (), Error = ()> + Send> {
        let mut guard = self.uncredited_settlement_amount.write();
        if let Some(leftovers) = (*guard).get_mut(&account_id) {
            if leftovers.1 > uncredited_settlement_amount.1 {
                // the current leftovers maintain the scale so we just need to
                // upscale the provided leftovers to the existing leftovers' scale
                let scaled = uncredited_settlement_amount
                    .0
                    .normalize_scale(ConvertDetails {
                        from: uncredited_settlement_amount.1,
                        to: leftovers.1,
                    })
                    .unwrap();
                *leftovers = (leftovers.0.clone() + scaled, leftovers.1);
            } else if leftovers.1 == uncredited_settlement_amount.1 {
                *leftovers = (
                    leftovers.0.clone() + uncredited_settlement_amount.0,
                    leftovers.1,
                );
            } else {
                // if the scale of the provided leftovers is bigger than
                // existing scale then we update the scale of the leftovers'
                // scale
                let scaled = leftovers
                    .0
                    .normalize_scale(ConvertDetails {
                        from: leftovers.1,
                        to: uncredited_settlement_amount.1,
                    })
                    .unwrap();
                *leftovers = (
                    uncredited_settlement_amount.0 + scaled,
                    uncredited_settlement_amount.1,
                );
            }
        } else {
            (*guard).insert(account_id, uncredited_settlement_amount);
        }
        Box::new(ok(()))
    }

    fn load_uncredited_settlement_amount(
        &self,
        account_id: Self::AccountId,
        local_scale: u8,
    ) -> Box<dyn Future<Item = Self::AssetType, Error = ()> + Send> {
        let mut guard = self.uncredited_settlement_amount.write();
        if let Some(l) = guard.get_mut(&account_id) {
            let ret = l.clone();
            let (scaled_leftover_amount, leftover_precision_loss) =
                scale_with_precision_loss(ret.0, local_scale, ret.1);
            // save the new leftovers
            *l = (leftover_precision_loss, std::cmp::max(local_scale, ret.1));
            Box::new(ok(scaled_leftover_amount))
        } else {
            Box::new(ok(BigUint::from(0u32)))
        }
    }

    fn get_uncredited_settlement_amount(
        &self,
        account_id: u64,
    ) -> Box<dyn Future<Item = (Self::AssetType, u8), Error = ()> + Send> {
        let leftovers = self.uncredited_settlement_amount.read();
        Box::new(ok(if let Some(a) = leftovers.get(&account_id) {
            a.clone()
        } else {
            (BigUint::from(0u32), 1)
        }))
    }
}

impl TestStore {
    pub fn new(accs: Vec<TestAccount>, should_fail: bool) -> Self {
        TestStore {
            accounts: Arc::new(RwLock::new(accs)),
            should_fail,
            cache: Arc::new(RwLock::new(HashMap::new())),
            cache_hits: Arc::new(RwLock::new(0)),
            uncredited_settlement_amount: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub fn get_balance(&self, account_id: u64) -> i64 {
        let accounts = &*self.accounts.read();
        for a in accounts {
            if a.id() == account_id {
                return a.balance;
            }
        }
        0
    }
}

// Test Service

impl TestAccount {
    pub fn new(id: u64, url: &str, ilp_address: &str) -> Self {
        Self {
            id,
            url: Url::parse(url).unwrap(),
            ilp_address: Address::from_str(ilp_address).unwrap(),
            no_details: false,
            balance: 0,
        }
    }
}

#[allow(dead_code)]
pub fn mock_settlement(status_code: usize) -> mockito::Mock {
    mock("POST", SETTLEMENT_API.clone())
        // The settlement API receives json data
        .match_header("Content-Type", "application/json")
        .with_status(status_code)
        .with_body(BODY)
}

pub fn mock_message(status_code: usize) -> mockito::Mock {
    mock("POST", MESSAGES_API.clone())
        // The messages API receives raw data
        .match_header("Content-Type", "application/octet-stream")
        .with_status(status_code)
        .with_body(BODY)
}

// Futures helper taken from the store_helpers in interledger-store-redis.
pub fn block_on<F>(f: F) -> Result<F::Item, F::Error>
where
    F: Future + Send + 'static,
    F::Item: Send,
    F::Error: Send,
{
    // Only run one test at a time
    let _ = env_logger::try_init();
    let mut runtime = Runtime::new().unwrap();
    runtime.block_on(f)
}

pub fn test_service(
) -> SettlementMessageService<impl IncomingService<TestAccount> + Clone, TestAccount> {
    SettlementMessageService::new(incoming_service_fn(|_request| {
        Box::new(err(RejectBuilder {
            code: ErrorCode::F02_UNREACHABLE,
            message: b"No other incoming handler!",
            data: &[],
            triggered_by: Some(&SERVICE_ADDRESS),
        }
        .build()))
    }))
}

pub fn test_store(store_fails: bool, account_has_engine: bool) -> TestStore {
    let mut acc = TEST_ACCOUNT_0.clone();
    acc.no_details = !account_has_engine;

    TestStore::new(vec![acc], store_fails)
}

pub fn test_api(
    test_store: TestStore,
    should_fulfill: bool,
) -> SettlementApi<TestStore, impl OutgoingService<TestAccount> + Clone + Send + Sync, TestAccount>
{
    let outgoing = outgoing_service_fn(move |_| {
        Box::new(if should_fulfill {
            ok(FulfillBuilder {
                fulfillment: &[0; 32],
                data: b"hello!",
            }
            .build())
        } else {
            err(RejectBuilder {
                code: ErrorCode::F02_UNREACHABLE,
                message: b"No other outgoing handler!",
                data: &[],
                triggered_by: Some(&SERVICE_ADDRESS),
            }
            .build())
        })
    });
    SettlementApi::new(test_store, outgoing)
}
