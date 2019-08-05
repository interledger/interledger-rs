use super::*;
use crate::SettlementEngineDetails;
use futures::{
    future::{err, ok},
    Future,
};
use interledger_ildcp::IldcpAccount;
use interledger_service::{
    incoming_service_fn, outgoing_service_fn, Account, AccountStore, IncomingService,
    OutgoingService,
};

use interledger_packet::{Address, ErrorCode, FulfillBuilder, RejectBuilder};
use mockito::mock;

use crate::fixtures::{BODY, MESSAGES_API, SERVICE_ADDRESS, SETTLEMENT_API, TEST_ACCOUNT_0};
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
}

impl Account for TestAccount {
    type AccountId = u64;

    fn id(&self) -> u64 {
        self.id
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

impl IldcpAccount for TestAccount {
    fn asset_code(&self) -> &str {
        "XYZ"
    }

    fn asset_scale(&self) -> u8 {
        9
    }

    fn client_address(&self) -> &Address {
        &self.ilp_address
    }
}

// Test Store
#[derive(Clone)]
pub struct TestStore {
    pub accounts: Arc<Vec<TestAccount>>,
    pub should_fail: bool,
    pub cache: Arc<RwLock<HashMap<String, IdempotentData>>>,
    pub cache_hits: Arc<RwLock<u64>>,
}

impl SettlementStore for TestStore {
    type Account = TestAccount;

    fn update_balance_for_incoming_settlement(
        &self,
        _account_id: <Self::Account as Account>::AccountId,
        _amount: u64,
        _idempotency_key: Option<String>,
    ) -> Box<Future<Item = (), Error = ()> + Send> {
        let ret = if self.should_fail { err(()) } else { ok(()) };
        Box::new(ret)
    }

    fn refund_settlement(
        &self,
        _account_id: <Self::Account as Account>::AccountId,
        _settle_amount: u64,
    ) -> Box<Future<Item = (), Error = ()> + Send> {
        let ret = if self.should_fail { err(()) } else { ok(()) };
        Box::new(ret)
    }
}

impl IdempotentStore for TestStore {
    fn load_idempotent_data(
        &self,
        idempotency_key: String,
    ) -> Box<dyn Future<Item = IdempotentData, Error = ()> + Send> {
        let cache = self.cache.read();
        if let Some(data) = cache.get(&idempotency_key) {
            let mut guard = self.cache_hits.write();
            *guard += 1; // used to test how many times this branch gets executed
            Box::new(ok((data.0, data.1.clone(), data.2)))
        } else {
            Box::new(err(()))
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

impl TestStore {
    pub fn new(accs: Vec<TestAccount>, should_fail: bool) -> Self {
        TestStore {
            accounts: Arc::new(accs),
            should_fail,
            cache: Arc::new(RwLock::new(HashMap::new())),
            cache_hits: Arc::new(RwLock::new(0)),
        }
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
    SettlementMessageService::new(
        SERVICE_ADDRESS.clone(),
        incoming_service_fn(|_request| {
            Box::new(err(RejectBuilder {
                code: ErrorCode::F02_UNREACHABLE,
                message: b"No other incoming handler!",
                data: &[],
                triggered_by: Some(&SERVICE_ADDRESS),
            }
            .build()))
        }),
    )
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
