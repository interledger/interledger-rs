/* kcov-ignore-start */
use super::*;
use crate::{packet::CCP_RESPONSE, server::CcpRouteManager};
use futures::{
    future::{err, ok},
    Future,
};
use interledger_packet::{Address, ErrorCode, RejectBuilder};
use interledger_service::{
    incoming_service_fn, outgoing_service_fn, AddressStore, BoxedIlpFuture, IncomingService,
    OutgoingRequest, OutgoingService, Username,
};
#[cfg(test)]
use lazy_static::lazy_static;
use parking_lot::Mutex;
use std::collections::HashMap;
use std::str::FromStr;
use std::{iter::FromIterator, sync::Arc};

lazy_static! {
    pub static ref ROUTING_ACCOUNT: TestAccount = TestAccount {
        id: Uuid::new_v4(),
        ilp_address: Address::from_str("example.peer").unwrap(),
        relation: RoutingRelation::Peer,
    };
    pub static ref NON_ROUTING_ACCOUNT: TestAccount = TestAccount {
        id: Uuid::new_v4(),
        ilp_address: Address::from_str("example.me.nonroutingaccount").unwrap(),
        relation: RoutingRelation::NonRoutingAccount,
    };
    pub static ref CHILD_ACCOUNT: TestAccount = TestAccount {
        id: Uuid::new_v4(),
        ilp_address: Address::from_str("example.me.child").unwrap(),
        relation: RoutingRelation::Child,
    };
    pub static ref EXAMPLE_CONNECTOR: Address = Address::from_str("example.connector").unwrap();
    pub static ref ALICE: Username = Username::from_str("alice").unwrap();
}

#[derive(Clone, Debug)]
pub struct TestAccount {
    pub id: Uuid,
    pub ilp_address: Address,
    pub relation: RoutingRelation,
}

impl TestAccount {
    pub fn new(id: Uuid, ilp_address: &str) -> TestAccount {
        TestAccount {
            id,
            ilp_address: Address::from_str(ilp_address).unwrap(),
            relation: RoutingRelation::Peer,
        }
    }
}

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

    fn asset_scale(&self) -> u8 {
        9
    }

    fn ilp_address(&self) -> &Address {
        &self.ilp_address
    }
}

impl CcpRoutingAccount for TestAccount {
    fn routing_relation(&self) -> RoutingRelation {
        self.relation
    }
}

#[derive(Clone)]
pub struct TestStore {
    pub local: HashMap<String, TestAccount>,
    pub configured: HashMap<String, TestAccount>,
    pub routes: Arc<Mutex<HashMap<String, TestAccount>>>,
}

impl TestStore {
    pub fn new() -> TestStore {
        TestStore {
            local: HashMap::new(),
            configured: HashMap::new(),
            routes: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub fn with_routes(
        local: HashMap<String, TestAccount>,
        configured: HashMap<String, TestAccount>,
    ) -> TestStore {
        TestStore {
            local,
            configured,
            routes: Arc::new(Mutex::new(HashMap::new())),
        }
    }
}

type RoutingTable<A> = HashMap<String, A>;

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

impl RouteManagerStore for TestStore {
    type Account = TestAccount;

    fn get_local_and_configured_routes(
        &self,
    ) -> Box<
        dyn Future<Item = (RoutingTable<TestAccount>, RoutingTable<TestAccount>), Error = ()>
            + Send,
    > {
        Box::new(ok((self.local.clone(), self.configured.clone())))
    }

    fn get_accounts_to_send_routes_to(
        &self,
        ignore_accounts: Vec<Uuid>,
    ) -> Box<dyn Future<Item = Vec<TestAccount>, Error = ()> + Send> {
        let mut accounts: Vec<TestAccount> = self
            .local
            .values()
            .chain(self.configured.values())
            .chain(self.routes.lock().values())
            .filter(|account| {
                account.should_send_routes() && !ignore_accounts.contains(&account.id)
            })
            .cloned()
            .collect();
        accounts.dedup_by_key(|a| a.id());
        Box::new(ok(accounts))
    }

    fn get_accounts_to_receive_routes_from(
        &self,
    ) -> Box<dyn Future<Item = Vec<TestAccount>, Error = ()> + Send> {
        let mut accounts: Vec<TestAccount> = self
            .local
            .values()
            .chain(self.configured.values())
            .chain(self.routes.lock().values())
            .filter(|account| account.should_receive_routes())
            .cloned()
            .collect();
        accounts.dedup_by_key(|a| a.id());
        Box::new(ok(accounts))
    }

    fn set_routes(
        &mut self,
        routes: impl IntoIterator<Item = (String, TestAccount)>,
    ) -> Box<dyn Future<Item = (), Error = ()> + Send> {
        *self.routes.lock() = HashMap::from_iter(routes.into_iter());
        Box::new(ok(()))
    }
}

pub fn test_service() -> CcpRouteManager<
    impl IncomingService<TestAccount, Future = BoxedIlpFuture> + Clone,
    impl OutgoingService<TestAccount, Future = BoxedIlpFuture> + Clone,
    TestStore,
    TestAccount,
> {
    let addr = Address::from_str("example.connector").unwrap();
    CcpRouteManagerBuilder::new(
        addr.clone(),
        TestStore::new(),
        outgoing_service_fn(|_request| {
            Box::new(err(RejectBuilder {
                code: ErrorCode::F02_UNREACHABLE,
                message: b"No other outgoing handler!",
                data: &[],
                triggered_by: Some(&EXAMPLE_CONNECTOR),
            }
            .build()))
        }),
        incoming_service_fn(|_request| {
            Box::new(err(RejectBuilder {
                code: ErrorCode::F02_UNREACHABLE,
                message: b"No other incoming handler!",
                data: &[],
                triggered_by: Some(&EXAMPLE_CONNECTOR),
            }
            .build()))
        }),
    )
    .ilp_address(addr)
    .to_service()
}

type OutgoingRequests = Arc<Mutex<Vec<OutgoingRequest<TestAccount>>>>;

pub fn test_service_with_routes() -> (
    CcpRouteManager<
        impl IncomingService<TestAccount, Future = BoxedIlpFuture> + Clone,
        impl OutgoingService<TestAccount, Future = BoxedIlpFuture> + Clone,
        TestStore,
        TestAccount,
    >,
    OutgoingRequests,
) {
    let local_routes = HashMap::from_iter(vec![
        (
            "example.local.1".to_string(),
            TestAccount::new(Uuid::from_slice(&[1; 16]).unwrap(), "example.local.1"),
        ),
        (
            "example.connector.other-local".to_string(),
            TestAccount {
                id: Uuid::from_slice(&[3; 16]).unwrap(),
                ilp_address: Address::from_str("example.connector.other-local").unwrap(),
                relation: RoutingRelation::NonRoutingAccount,
            },
        ),
    ]);
    let configured_routes = HashMap::from_iter(vec![(
        "example.configured.1".to_string(),
        TestAccount::new(Uuid::from_slice(&[2; 16]).unwrap(), "example.configured.1"),
    )]);
    let store = TestStore::with_routes(local_routes, configured_routes);
    let outgoing_requests: Arc<Mutex<Vec<OutgoingRequest<TestAccount>>>> =
        Arc::new(Mutex::new(Vec::new()));
    let outgoing_requests_clone = outgoing_requests.clone();
    let outgoing = outgoing_service_fn(move |request: OutgoingRequest<TestAccount>| {
        (*outgoing_requests_clone.lock()).push(request);
        Ok(CCP_RESPONSE.clone())
    });
    let addr = Address::from_str("example.connector").unwrap();
    let service = CcpRouteManagerBuilder::new(
        addr.clone(),
        store,
        outgoing,
        incoming_service_fn(|_request| {
            Box::new(err(RejectBuilder {
                code: ErrorCode::F02_UNREACHABLE,
                message: b"No other incoming handler!",
                data: &[],
                triggered_by: Some(&EXAMPLE_CONNECTOR),
            }
            .build()))
        }),
    )
    .ilp_address(addr)
    .to_service();
    (service, outgoing_requests)
}
/* kcov-ignore-end */
