use super::*;
use crate::core::{
    idempotency::*,
    scale_with_precision_loss,
    types::{
        Convert, ConvertDetails, LeftoversStore, SettlementAccount, SettlementEngineDetails,
        SettlementStore,
    },
};
use bytes::Bytes;
use hyper::StatusCode;
use interledger_packet::{Address, ErrorCode, FulfillBuilder, RejectBuilder};
use interledger_service::{
    incoming_service_fn, outgoing_service_fn, Account, AccountStore, IncomingService, Username,
};
use mockito::mock;
use num_bigint::BigUint;
use uuid::Uuid;

use super::fixtures::{BODY, MESSAGES_API, SERVICE_ADDRESS, TEST_ACCOUNT_0};
use async_trait::async_trait;
use interledger_errors::*;
use once_cell::sync::Lazy;
use parking_lot::RwLock;
use std::cmp::Ordering;
use std::collections::HashMap;
use std::str::FromStr;
use std::sync::Arc;
use url::Url;

#[derive(Debug, Clone)]
pub struct TestAccount {
    pub id: Uuid,
    pub url: Url,
    pub ilp_address: Address,
    pub no_details: bool,
    pub balance: i64,
}

pub static ALICE: Lazy<Username> = Lazy::new(|| Username::from_str("alice").unwrap());

impl Account for TestAccount {
    fn id(&self) -> Uuid {
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
    pub uncredited_settlement_amount: Arc<RwLock<HashMap<Uuid, (BigUint, u8)>>>,
}

#[async_trait]
impl SettlementStore for TestStore {
    type Account = TestAccount;

    async fn update_balance_for_incoming_settlement(
        &self,
        account_id: Uuid,
        amount: u64,
        _idempotency_key: Option<String>,
    ) -> Result<(), SettlementStoreError> {
        let mut accounts = self.accounts.write();
        for mut a in &mut *accounts {
            if a.id() == account_id {
                a.balance += amount as i64;
            }
        }
        if self.should_fail {
            Err(SettlementStoreError::BalanceUpdateFailure)
        } else {
            Ok(())
        }
    }

    async fn refund_settlement(
        &self,
        _account_id: Uuid,
        _settle_amount: u64,
    ) -> Result<(), SettlementStoreError> {
        if self.should_fail {
            Err(SettlementStoreError::RefundFailure)
        } else {
            Ok(())
        }
    }
}

#[async_trait]
impl IdempotentStore for TestStore {
    async fn load_idempotent_data(
        &self,
        idempotency_key: String,
    ) -> Result<Option<IdempotentData>, IdempotentStoreError> {
        let cache = self.cache.read();
        if let Some(data) = cache.get(&idempotency_key) {
            let mut guard = self.cache_hits.write();
            *guard += 1; // used to test how many times this branch gets executed
            Ok(Some(data.clone()))
        } else {
            Ok(None)
        }
    }

    async fn save_idempotent_data(
        &self,
        idempotency_key: String,
        input_hash: [u8; 32],
        status_code: StatusCode,
        data: Bytes,
    ) -> Result<(), IdempotentStoreError> {
        let mut cache = self.cache.write();
        cache.insert(
            idempotency_key,
            IdempotentData::new(status_code, data, input_hash),
        );
        Ok(())
    }
}

#[async_trait]
impl AccountStore for TestStore {
    type Account = TestAccount;

    async fn get_accounts(
        &self,
        account_ids: Vec<Uuid>,
    ) -> Result<Vec<Self::Account>, AccountStoreError> {
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
            Ok(accounts)
        } else {
            Err(AccountStoreError::WrongLength {
                expected: account_ids.len(),
                actual: accounts.len(),
            })
        }
    }

    // stub implementation (not used in these tests)
    async fn get_account_id_from_username(
        &self,
        _username: &Username,
    ) -> Result<Uuid, AccountStoreError> {
        Ok(Uuid::new_v4())
    }
}

#[async_trait]
impl LeftoversStore for TestStore {
    type AccountId = Uuid;
    type AssetType = BigUint;

    async fn save_uncredited_settlement_amount(
        &self,
        account_id: Uuid,
        uncredited_settlement_amount: (Self::AssetType, u8),
    ) -> Result<(), LeftoversStoreError> {
        let mut guard = self.uncredited_settlement_amount.write();
        if let Some(leftovers) = (*guard).get_mut(&account_id) {
            match leftovers.1.cmp(&uncredited_settlement_amount.1) {
                Ordering::Greater => {
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
                }
                Ordering::Equal => {
                    *leftovers = (
                        leftovers.0.clone() + uncredited_settlement_amount.0,
                        leftovers.1,
                    );
                }
                _ => {
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
            }
        } else {
            (*guard).insert(account_id, uncredited_settlement_amount);
        }
        Ok(())
    }

    async fn load_uncredited_settlement_amount(
        &self,
        account_id: Uuid,
        local_scale: u8,
    ) -> Result<Self::AssetType, LeftoversStoreError> {
        let mut guard = self.uncredited_settlement_amount.write();
        if let Some(l) = guard.get_mut(&account_id) {
            let ret = l.clone();
            let (scaled_leftover_amount, leftover_precision_loss) =
                scale_with_precision_loss(ret.0, local_scale, ret.1);
            // save the new leftovers
            *l = (leftover_precision_loss, std::cmp::max(local_scale, ret.1));
            Ok(scaled_leftover_amount)
        } else {
            Ok(BigUint::from(0u32))
        }
    }

    async fn get_uncredited_settlement_amount(
        &self,
        account_id: Uuid,
    ) -> Result<(Self::AssetType, u8), LeftoversStoreError> {
        let leftovers = self.uncredited_settlement_amount.read();
        Ok(if let Some(a) = leftovers.get(&account_id) {
            a.clone()
        } else {
            (BigUint::from(0u32), 1)
        })
    }

    async fn clear_uncredited_settlement_amount(
        &self,
        _account_id: Uuid,
    ) -> Result<(), LeftoversStoreError> {
        unreachable!()
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

    pub fn get_balance(&self, account_id: Uuid) -> i64 {
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
    pub fn new(id: Uuid, url: &str, ilp_address: &str) -> Self {
        Self {
            id,
            url: Url::parse(url).unwrap(),
            ilp_address: Address::from_str(ilp_address).unwrap(),
            no_details: false,
            balance: 0,
        }
    }
}

pub fn mock_message(status_code: usize) -> mockito::Mock {
    mock("POST", MESSAGES_API.clone())
        // The messages API receives raw data
        .match_header("Content-Type", "application/octet-stream")
        .with_status(status_code)
        .with_body(BODY)
}

pub fn test_service(
) -> SettlementMessageService<impl IncomingService<TestAccount> + Clone, TestAccount> {
    SettlementMessageService::new(incoming_service_fn(|_request| {
        Err(RejectBuilder {
            code: ErrorCode::F02_UNREACHABLE,
            message: b"No other incoming handler!",
            data: &[],
            triggered_by: Some(&SERVICE_ADDRESS),
        }
        .build())
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
) -> warp::filters::BoxedFilter<(impl warp::Reply,)> {
    let outgoing = outgoing_service_fn(move |_| {
        if should_fulfill {
            Ok(FulfillBuilder {
                fulfillment: &[0; 32],
                data: b"hello!",
            }
            .build())
        } else {
            Err(RejectBuilder {
                code: ErrorCode::F02_UNREACHABLE,
                message: b"No other outgoing handler!",
                data: &[],
                triggered_by: Some(&SERVICE_ADDRESS),
            }
            .build())
        }
    });
    create_settlements_filter(test_store, outgoing)
}
