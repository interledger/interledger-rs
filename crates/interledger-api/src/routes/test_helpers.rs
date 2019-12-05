use crate::{
    routes::{accounts_api, node_settings_api},
    AccountDetails, AccountSettings, NodeStore,
};
use bytes::Bytes;
use futures::sync::mpsc::UnboundedSender;
use futures::{
    future::{err, ok},
    Future,
};
use http::Response;
use interledger_btp::{BtpAccount, BtpOutgoingService};
use interledger_ccp::{CcpRoutingAccount, RoutingRelation};
use interledger_http::error::default_rejection_handler;
use interledger_http::{HttpAccount, HttpStore};
use interledger_packet::{Address, ErrorCode, FulfillBuilder, RejectBuilder};
use interledger_router::RouterStore;
use interledger_service::{
    incoming_service_fn, outgoing_service_fn, Account, AccountStore, AddressStore, Username,
};
use interledger_service_util::{BalanceStore, ExchangeRateStore};
use interledger_settlement::core::types::{SettlementAccount, SettlementEngineDetails};
use interledger_stream::{PaymentNotification, StreamNotificationsStore};
use lazy_static::lazy_static;
use secrecy::SecretString;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::str::FromStr;
use std::sync::Arc;
use url::Url;
use uuid::Uuid;
use warp::{self, Filter};

pub fn api_call<F, T: ToString>(
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

    ret.reply(api)
}

pub fn test_node_settings_api(
) -> impl warp::Filter<Extract = (impl warp::Reply,), Error = warp::Rejection> + Clone {
    node_settings_api("admin".to_owned(), None, TestStore).recover(default_rejection_handler)
}

pub fn test_accounts_api(
) -> impl warp::Filter<Extract = (impl warp::Reply,), Error = warp::Rejection> + Clone {
    let incoming = incoming_service_fn(|_request| {
        Box::new(err(RejectBuilder {
            code: ErrorCode::F02_UNREACHABLE,
            message: b"No other incoming handler!",
            data: &[],
            triggered_by: None,
        }
        .build()))
    });
    let outgoing = outgoing_service_fn(move |_request| {
        Box::new(ok(FulfillBuilder {
            fulfillment: &[0; 32],
            data: b"hello!",
        }
        .build()))
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
lazy_static! {
    pub static ref USERNAME: Username = Username::from_str("alice").unwrap();
    pub static ref EXAMPLE_ADDRESS: Address = Address::from_str("example.alice").unwrap();
    pub static ref DETAILS: Option<Value> = Some(json!({
        "ilp_address": "example.alice",
        "username": "alice",
        "asset_code": "XYZ",
        "asset_scale": 9,
        "ilp_over_http_incoming_token" : "password",
    }));
}
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

impl AccountStore for TestStore {
    type Account = TestAccount;

    fn get_accounts(
        &self,
        _account_ids: Vec<Uuid>,
    ) -> Box<dyn Future<Item = Vec<TestAccount>, Error = ()> + Send> {
        Box::new(ok(vec![TestAccount]))
    }

    // stub implementation (not used in these tests)
    fn get_account_id_from_username(
        &self,
        _username: &Username,
    ) -> Box<dyn Future<Item = Uuid, Error = ()> + Send> {
        Box::new(ok(Uuid::new_v4()))
    }
}

impl ExchangeRateStore for TestStore {
    fn get_exchange_rates(&self, _asset_codes: &[&str]) -> Result<Vec<f64>, ()> {
        Ok(vec![1.0, 2.0])
    }

    fn set_exchange_rates(&self, _rates: HashMap<String, f64>) -> Result<(), ()> {
        Ok(())
    }

    fn get_all_exchange_rates(&self) -> Result<HashMap<String, f64>, ()> {
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

impl NodeStore for TestStore {
    type Account = TestAccount;

    fn insert_account(
        &self,
        _account: AccountDetails,
    ) -> Box<dyn Future<Item = Self::Account, Error = ()> + Send> {
        Box::new(ok(TestAccount))
    }

    fn delete_account(
        &self,
        _id: Uuid,
    ) -> Box<dyn Future<Item = Self::Account, Error = ()> + Send> {
        Box::new(ok(TestAccount))
    }

    fn update_account(
        &self,
        _id: Uuid,
        _account: AccountDetails,
    ) -> Box<dyn Future<Item = Self::Account, Error = ()> + Send> {
        Box::new(ok(TestAccount))
    }

    fn modify_account_settings(
        &self,
        _id: Uuid,
        _settings: AccountSettings,
    ) -> Box<dyn Future<Item = Self::Account, Error = ()> + Send> {
        Box::new(ok(TestAccount))
    }

    fn get_all_accounts(&self) -> Box<dyn Future<Item = Vec<Self::Account>, Error = ()> + Send> {
        Box::new(ok(vec![TestAccount, TestAccount]))
    }

    fn set_static_routes<R>(&self, _routes: R) -> Box<dyn Future<Item = (), Error = ()> + Send>
    where
        R: IntoIterator<Item = (String, Uuid)>,
    {
        Box::new(ok(()))
    }

    fn set_static_route(
        &self,
        _prefix: String,
        _account_id: Uuid,
    ) -> Box<dyn Future<Item = (), Error = ()> + Send> {
        Box::new(ok(()))
    }

    fn set_default_route(
        &self,
        _account_id: Uuid,
    ) -> Box<dyn Future<Item = (), Error = ()> + Send> {
        unimplemented!()
    }

    fn set_settlement_engines(
        &self,
        _asset_to_url_map: impl IntoIterator<Item = (String, Url)>,
    ) -> Box<dyn Future<Item = (), Error = ()> + Send> {
        Box::new(ok(()))
    }

    fn get_asset_settlement_engine(
        &self,
        _asset_code: &str,
    ) -> Box<dyn Future<Item = Option<Url>, Error = ()> + Send> {
        Box::new(ok(None))
    }
}

impl AddressStore for TestStore {
    /// Saves the ILP Address in the store's memory and database
    fn set_ilp_address(
        &self,
        _ilp_address: Address,
    ) -> Box<dyn Future<Item = (), Error = ()> + Send> {
        unimplemented!()
    }

    fn clear_ilp_address(&self) -> Box<dyn Future<Item = (), Error = ()> + Send> {
        unimplemented!()
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

impl BalanceStore for TestStore {
    fn get_balance(&self, _account: TestAccount) -> Box<dyn Future<Item = i64, Error = ()> + Send> {
        Box::new(ok(1))
    }

    fn update_balances_for_prepare(
        &self,
        _from_account: TestAccount,
        _incoming_amount: u64,
    ) -> Box<dyn Future<Item = (), Error = ()> + Send> {
        unimplemented!()
    }

    fn update_balances_for_fulfill(
        &self,
        _to_account: TestAccount,
        _outgoing_amount: u64,
    ) -> Box<dyn Future<Item = (i64, u64), Error = ()> + Send> {
        unimplemented!()
    }

    fn update_balances_for_reject(
        &self,
        _from_account: TestAccount,
        _incoming_amount: u64,
    ) -> Box<dyn Future<Item = (), Error = ()> + Send> {
        unimplemented!()
    }
}

impl HttpStore for TestStore {
    type Account = TestAccount;
    fn get_account_from_http_auth(
        &self,
        username: &Username,
        token: &str,
    ) -> Box<dyn Future<Item = Self::Account, Error = ()> + Send> {
        if username == &*USERNAME && token == AUTH_PASSWORD {
            Box::new(ok(TestAccount))
        } else {
            Box::new(err(()))
        }
    }
}
