use super::*;
use crate::server::CcpServerService;
use bytes::Bytes;
use futures::{
    future::{err, ok},
    Future,
};
use hashbrown::HashMap;
use interledger_packet::{ErrorCode, RejectBuilder};
use interledger_service::{incoming_service_fn, BoxedIlpFuture, IncomingService};

lazy_static! {
    pub static ref ROUTING_ACCOUNT: TestAccount = TestAccount {
        id: 1,
        ilp_address: Bytes::from("example.peer"),
        send_routes: true,
        receive_routes: true,
        relation: RoutingRelation::Peer,
    };
    pub static ref NON_ROUTING_ACCOUNT: TestAccount = TestAccount {
        id: 1,
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

impl RoutingAccount for TestAccount {
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
pub struct TestStore {}

impl TestStore {
    pub fn new() -> TestStore {
        TestStore {}
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
        Box::new(ok((HashMap::new(), HashMap::new())))
    }
}

pub fn test_service() -> CcpServerService<
    impl IncomingService<TestAccount, Future = BoxedIlpFuture>,
    TestStore,
    TestAccount,
> {
    CcpServerService::with_spawn_bool(
        Bytes::from("example.connector"),
        TestStore::new(),
        incoming_service_fn(|_request| {
            Box::new(err(RejectBuilder {
                code: ErrorCode::F02_UNREACHABLE,
                message: b"No other handler!",
                data: &[],
                triggered_by: b"example.connector",
            }
            .build()))
        }),
        false,
    )
}
