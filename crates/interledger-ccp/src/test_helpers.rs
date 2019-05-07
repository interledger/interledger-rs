/* kcov-ignore-start */
use super::*;
use crate::{packet::CCP_RESPONSE, server::CcpRouteManager};
use bytes::Bytes;
use futures::{
    future::{err, ok},
    Future,
};
use hashbrown::HashMap;
use interledger_packet::{ErrorCode, RejectBuilder};
use interledger_service::{
    incoming_service_fn, outgoing_service_fn, BoxedIlpFuture, IncomingService, OutgoingRequest,
    OutgoingService,
};
use parking_lot::Mutex;
use std::{iter::FromIterator, sync::Arc};

lazy_static! {
    pub static ref ROUTING_ACCOUNT: TestAccount = TestAccount {
        id: 1,
        ilp_address: Bytes::from("example.peer"),
        send_routes: true,
        receive_routes: true,
        relation: RoutingRelation::Peer,
    };
    pub static ref NON_ROUTING_ACCOUNT: TestAccount = TestAccount {
        id: 2,
        ilp_address: Bytes::from("example.me.child"),
        send_routes: false,
        receive_routes: false,
        relation: RoutingRelation::Child,
    };
}

#[derive(Clone, Debug)]
pub struct TestAccount {
    pub id: u64,
    pub ilp_address: Bytes,
    pub receive_routes: bool,
    pub send_routes: bool,
    pub relation: RoutingRelation,
}

impl TestAccount {
    pub fn new(id: u64, ilp_address: &str) -> TestAccount {
        TestAccount {
            id,
            ilp_address: Bytes::from(ilp_address),
            receive_routes: true,
            send_routes: true,
            relation: RoutingRelation::Peer,
        }
    }
}

impl Account for TestAccount {
    type AccountId = u64;

    fn id(&self) -> u64 {
        self.id
    }
}

impl IldcpAccount for TestAccount {
    fn asset_code(&self) -> &str {
        "XYZ"
    }

    fn asset_scale(&self) -> u8 {
        9
    }

    fn client_address(&self) -> &[u8] {
        self.ilp_address.as_ref()
    }
}

impl CcpRoutingAccount for TestAccount {
    fn routing_relation(&self) -> RoutingRelation {
        self.relation
    }

    fn should_receive_routes(&self) -> bool {
        self.receive_routes
    }

    fn should_send_routes(&self) -> bool {
        self.send_routes
    }
}

#[derive(Clone)]
pub struct TestStore {
    pub local: HashMap<Bytes, TestAccount>,
    pub configured: HashMap<Bytes, TestAccount>,
    pub routes: Arc<Mutex<HashMap<Bytes, TestAccount>>>,
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
        local: HashMap<Bytes, TestAccount>,
        configured: HashMap<Bytes, TestAccount>,
    ) -> TestStore {
        TestStore {
            local: local,
            configured: configured,
            routes: Arc::new(Mutex::new(HashMap::new())),
        }
    }
}

impl RouteManagerStore for TestStore {
    type Account = TestAccount;

    fn get_local_and_configured_routes(
        &self,
    ) -> Box<
        Future<Item = (HashMap<Bytes, TestAccount>, HashMap<Bytes, TestAccount>), Error = ()>
            + Send,
    > {
        Box::new(ok((self.local.clone(), self.configured.clone())))
    }

    fn get_accounts_to_send_routes_to(
        &self,
    ) -> Box<Future<Item = Vec<TestAccount>, Error = ()> + Send> {
        let mut accounts: Vec<TestAccount> = self
            .local
            .values()
            .chain(self.configured.values())
            .chain(self.routes.lock().values())
            .filter(|account| account.send_routes)
            .cloned()
            .collect();
        accounts.dedup_by_key(|a| a.id());
        Box::new(ok(accounts))
    }

    fn get_accounts_to_receive_routes_from(
        &self,
    ) -> Box<Future<Item = Vec<TestAccount>, Error = ()> + Send> {
        let mut accounts: Vec<TestAccount> = self
            .local
            .values()
            .chain(self.configured.values())
            .chain(self.routes.lock().values())
            .filter(|account| account.receive_routes)
            .cloned()
            .collect();
        accounts.dedup_by_key(|a| a.id());
        Box::new(ok(accounts))
    }

    fn set_routes<R>(&mut self, routes: R) -> Box<Future<Item = (), Error = ()> + Send>
    where
        R: IntoIterator<Item = (Bytes, TestAccount)>,
    {
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
    CcpRouteManager::with_spawn_bool(
        TestAccount::new(0, "example.connector"),
        TestStore::new(),
        outgoing_service_fn(|_request| {
            Box::new(err(RejectBuilder {
                code: ErrorCode::F02_UNREACHABLE,
                message: b"No other outgoing handler!",
                data: &[],
                triggered_by: b"example.connector",
            }
            .build()))
        }),
        incoming_service_fn(|_request| {
            Box::new(err(RejectBuilder {
                code: ErrorCode::F02_UNREACHABLE,
                message: b"No other incoming handler!",
                data: &[],
                triggered_by: b"example.connector",
            }
            .build()))
        }),
        false,
    )
}

pub fn test_service_with_routes() -> (
    CcpRouteManager<
        impl IncomingService<TestAccount, Future = BoxedIlpFuture> + Clone,
        impl OutgoingService<TestAccount, Future = BoxedIlpFuture> + Clone,
        TestStore,
        TestAccount,
    >,
    Arc<Mutex<Vec<OutgoingRequest<TestAccount>>>>,
) {
    let local_routes = HashMap::from_iter(vec![
        (
            Bytes::from("example.local.1"),
            TestAccount::new(1, "example.local.1"),
        ),
        (
            Bytes::from("example.connector.other-local"),
            TestAccount {
                id: 3,
                ilp_address: Bytes::from("example.connector.other-local"),
                send_routes: false,
                receive_routes: false,
                relation: RoutingRelation::Child,
            },
        ),
    ]);
    let configured_routes = HashMap::from_iter(vec![(
        Bytes::from("example.configured.1"),
        TestAccount::new(2, "example.configured.1"),
    )]);
    let store = TestStore::with_routes(local_routes, configured_routes);
    let outgoing_requests: Arc<Mutex<Vec<OutgoingRequest<TestAccount>>>> =
        Arc::new(Mutex::new(Vec::new()));
    let outgoing_requests_clone = outgoing_requests.clone();
    let outgoing = outgoing_service_fn(move |request: OutgoingRequest<TestAccount>| {
        (*outgoing_requests_clone.lock()).push(request);
        Ok(CCP_RESPONSE.clone())
    });
    let service = CcpRouteManager::with_spawn_bool(
        TestAccount::new(0, "example.connector"),
        store,
        outgoing,
        incoming_service_fn(|_request| {
            Box::new(err(RejectBuilder {
                code: ErrorCode::F02_UNREACHABLE,
                message: b"No other incoming handler!",
                data: &[],
                triggered_by: b"example.connector",
            }
            .build()))
        }),
        false,
    );
    (service, outgoing_requests)
}
/* kcov-ignore-end */
