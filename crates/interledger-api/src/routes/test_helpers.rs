use crate::{
    routes::{accounts_api, node_settings_api},
    AccountDetails, AccountSettings, NodeStore,
};
use async_trait::async_trait;
use bytes::Bytes;
use futures::channel::mpsc::UnboundedSender;
use http::Response;
use interledger_btp::{BtpAccount, BtpOutgoingService};
use interledger_ccp::{CcpRoutingAccount, RoutingRelation};
use interledger_errors::*;
use interledger_http::{HttpAccount, HttpStore};
use interledger_packet::{Address, ErrorCode, FulfillBuilder, RejectBuilder};
use interledger_router::RouterStore;
use interledger_service::{
    incoming_service_fn, outgoing_service_fn, Account, AccountStore, AddressStore, Username,
};
use interledger_service_util::{BalanceStore, ExchangeRateStore};
use interledger_settlement::core::types::{SettlementAccount, SettlementEngineDetails};
use interledger_stream::{PaymentNotification, StreamNotificationsStore};
use once_cell::sync::Lazy;
use secrecy::SecretString;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::str::FromStr;
use std::sync::Arc;
use url::Url;
use uuid::Uuid;
use warp::{self, Filter};

pub async fn api_call<F, T: ToString>(
    api: &F,
    method: &str,
    endpoint: &str, // /ilp or /accounts/:username/ilp
    auth: T,        // simple bearer or overloaded username+password
    data: Option<Value>,
) -> Response<Bytes>
where
    F: warp::Filter + 'static,
    F::Extract: warp::Reply,
{
    let mut ret = warp::test::request()
        .method(method)
        .path(endpoint)
        .header("Authorization", format!("Bearer {}", auth.to_string()));

    if let Some(d) = data {
        ret = ret.header("Content-type", "application/json").json(&d);
    }

    ret.reply(api).await
}

pub fn test_node_settings_api(
) -> impl warp::Filter<Extract = (impl warp::Reply,), Error = warp::Rejection> + Clone {
    node_settings_api("admin".to_owned(), None, TestStore).recover(default_rejection_handler)
}

pub fn test_accounts_api(
) -> impl warp::Filter<Extract = (impl warp::Reply,), Error = warp::Rejection> + Clone {
    let incoming = incoming_service_fn(|_request| {
        Err(RejectBuilder {
            code: ErrorCode::F02_UNREACHABLE,
            message: b"No other incoming handler!",
            data: &[],
            triggered_by: None,
        }
        .build())
    });
    let outgoing = outgoing_service_fn(move |_request| {
        Ok(FulfillBuilder {
            fulfillment: &[0; 32],
            data: b"hello!",
        }
        .build())
    });
    let btp = BtpOutgoingService::new(
        Address::from_str("example.alice").unwrap(),
        outgoing.clone(),
    );
    let store = TestStore;
    accounts_api(
        Bytes::from("admin"),
        "admin".to_owned(),
        None,
        incoming,
        outgoing,
        btp,
        store,
    )
    .recover(default_rejection_handler)
}

/*
 * Lots of boilerplate implementations of all necessary traits to launch
 * the crate's APIs in unit tests
 */

#[derive(Clone)]
struct TestStore;

use serde_json::json;
pub static USERNAME: Lazy<Username> = Lazy::new(|| Username::from_str("alice").unwrap());
pub static EXAMPLE_ADDRESS: Lazy<Address> =
    Lazy::new(|| Address::from_str("example.alice").unwrap());
pub static DETAILS: Lazy<Option<Value>> = Lazy::new(|| {
    Some(json!({
        "ilp_address": "example.alice",
        "username": "alice",
        "asset_code": "XYZ",
        "asset_scale": 9,
        "ilp_over_http_incoming_token" : "password",
    }))
});
const AUTH_PASSWORD: &str = "password";

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct TestAccount;

impl Account for TestAccount {
    fn id(&self) -> Uuid {
        Uuid::new_v4()
    }

    fn username(&self) -> &Username {
        &USERNAME
    }

    fn asset_scale(&self) -> u8 {
        9
    }

    fn asset_code(&self) -> &str {
        "XYZ"
    }

    fn ilp_address(&self) -> &Address {
        &EXAMPLE_ADDRESS
    }
}

impl HttpAccount for TestAccount {
    fn get_http_auth_token(&self) -> Option<SecretString> {
        unimplemented!()
    }

    fn get_http_url(&self) -> Option<&Url> {
        unimplemented!()
    }
}

impl BtpAccount for TestAccount {
    fn get_ilp_over_btp_url(&self) -> Option<&Url> {
        None
    }

    fn get_ilp_over_btp_outgoing_token(&self) -> Option<&[u8]> {
        unimplemented!()
    }
}

impl SettlementAccount for TestAccount {
    fn settlement_engine_details(&self) -> Option<SettlementEngineDetails> {
        None
    }
}

impl CcpRoutingAccount for TestAccount {
    fn routing_relation(&self) -> RoutingRelation {
        RoutingRelation::NonRoutingAccount
    }
}

#[async_trait]
impl AccountStore for TestStore {
    type Account = TestAccount;

    async fn get_accounts(
        &self,
        _account_ids: Vec<Uuid>,
    ) -> Result<Vec<TestAccount>, AccountStoreError> {
        Ok(vec![TestAccount])
    }

    // stub implementation (not used in these tests)
    async fn get_account_id_from_username(
        &self,
        _username: &Username,
    ) -> Result<Uuid, AccountStoreError> {
        Ok(Uuid::new_v4())
    }
}

impl ExchangeRateStore for TestStore {
    fn get_exchange_rates(
        &self,
        _asset_codes: &[&str],
    ) -> Result<Vec<f64>, ExchangeRateStoreError> {
        Ok(vec![1.0, 2.0])
    }

    fn set_exchange_rates(
        &self,
        _rates: HashMap<String, f64>,
    ) -> Result<(), ExchangeRateStoreError> {
        Ok(())
    }

    fn get_all_exchange_rates(&self) -> Result<HashMap<String, f64>, ExchangeRateStoreError> {
        let mut ret = HashMap::new();
        ret.insert("ABC".to_owned(), 1.0);
        ret.insert("XYZ".to_owned(), 2.0);
        Ok(ret)
    }
}

impl RouterStore for TestStore {
    fn routing_table(&self) -> Arc<HashMap<String, Uuid>> {
        Arc::new(HashMap::new())
    }
}

#[async_trait]
impl NodeStore for TestStore {
    type Account = TestAccount;

    async fn insert_account(
        &self,
        _account: AccountDetails,
    ) -> Result<Self::Account, NodeStoreError> {
        Ok(TestAccount)
    }

    async fn delete_account(&self, _id: Uuid) -> Result<Self::Account, NodeStoreError> {
        Ok(TestAccount)
    }

    async fn update_account(
        &self,
        _id: Uuid,
        _account: AccountDetails,
    ) -> Result<Self::Account, NodeStoreError> {
        Ok(TestAccount)
    }

    async fn modify_account_settings(
        &self,
        _id: Uuid,
        _settings: AccountSettings,
    ) -> Result<Self::Account, NodeStoreError> {
        Ok(TestAccount)
    }

    async fn get_all_accounts(&self) -> Result<Vec<Self::Account>, NodeStoreError> {
        Ok(vec![TestAccount, TestAccount])
    }

    async fn set_static_routes<R>(&self, _routes: R) -> Result<(), NodeStoreError>
    where
        R: IntoIterator<Item = (String, Uuid)> + Send + 'async_trait,
    {
        Ok(())
    }

    async fn set_static_route(
        &self,
        _prefix: String,
        _account_id: Uuid,
    ) -> Result<(), NodeStoreError> {
        Ok(())
    }

    async fn set_default_route(&self, _account_id: Uuid) -> Result<(), NodeStoreError> {
        unimplemented!()
    }

    async fn set_settlement_engines(
        &self,
        _asset_to_url_map: impl IntoIterator<Item = (String, Url)> + Send + 'async_trait,
    ) -> Result<(), NodeStoreError> {
        Ok(())
    }

    async fn get_asset_settlement_engine(
        &self,
        _asset_code: &str,
    ) -> Result<Option<Url>, NodeStoreError> {
        Ok(None)
    }
}

#[async_trait]
impl AddressStore for TestStore {
    /// Saves the ILP Address in the store's memory and database
    async fn set_ilp_address(&self, _ilp_address: Address) -> Result<(), AddressStoreError> {
        Ok(())
    }

    async fn clear_ilp_address(&self) -> Result<(), AddressStoreError> {
        Ok(())
    }

    /// Get's the store's ilp address from memory
    fn get_ilp_address(&self) -> Address {
        Address::from_str("example.connector").unwrap()
    }
}

impl StreamNotificationsStore for TestStore {
    type Account = TestAccount;

    fn add_payment_notification_subscription(
        &self,
        _id: Uuid,
        _sender: UnboundedSender<PaymentNotification>,
    ) {
        unimplemented!()
    }

    fn publish_payment_notification(&self, _payment: PaymentNotification) {
        unimplemented!()
    }
}

#[async_trait]
impl BalanceStore for TestStore {
    async fn get_balance(&self, _: Uuid) -> Result<i64, BalanceStoreError> {
        Ok(1)
    }

    async fn update_balances_for_prepare(
        &self,
        _: Uuid,
        _incoming_amount: u64,
    ) -> Result<(), BalanceStoreError> {
        unimplemented!()
    }

    async fn update_balances_for_fulfill(
        &self,
        _: Uuid,
        _outgoing_amount: u64,
    ) -> Result<(i64, u64), BalanceStoreError> {
        unimplemented!()
    }

    async fn update_balances_for_reject(
        &self,
        _: Uuid,
        _incoming_amount: u64,
    ) -> Result<(), BalanceStoreError> {
        unimplemented!()
    }
}

#[async_trait]
impl HttpStore for TestStore {
    type Account = TestAccount;
    async fn get_account_from_http_auth(
        &self,
        username: &Username,
        token: &str,
    ) -> Result<Self::Account, HttpStoreError> {
        if username == &*USERNAME && token == AUTH_PASSWORD {
            Ok(TestAccount)
        } else {
            Err(HttpStoreError::Unauthorized(username.to_string()))
        }
    }
}
