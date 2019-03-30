use crate::{packet::*, routing_table::RoutingTable, CcpAccount};
use bytes::Bytes;
use futures::{
    future::{err, ok},
    Future,
};
use hashbrown::HashMap;
use interledger_packet::*;
use interledger_service::{Account, BoxedIlpFuture, IncomingRequest, IncomingService};
use parking_lot::RwLock;
use std::{marker::PhantomData, sync::Arc};
use tokio_executor::spawn;

/// The Routing Manager Service.
///
/// This implements the Connector-to-Connector Protocol (CCP)
/// for exchanging route updates with peers. This service handles incoming CCP messages
/// and sends updates to peers. It manages the routing table in the Store and updates it
/// with the best routes determined by per-account configuration and the broadcasts we have
/// received from peers.
#[derive(Clone)]
pub struct CcpServerService<S, A: Account> {
    ilp_address: Bytes,
    global_prefix: Bytes,
    /// The next incoming request handler. This will be used both to pass on requests that are
    /// not CCP messages AND to send outgoing CCP messages to peers.
    next: S,
    account_type: PhantomData<A>,
    /// This represents the routing table we will forward to our peers.
    /// It is the same as the local_table with our own address added to the path of each route.
    forwarding_table: Arc<RwLock<RoutingTable>>,
    /// This is the routing table we have compile from configuration and
    /// broadcasts we have received from our peers. It is saved to the Store so that
    /// the Router services forwards packets according to what it says.
    local_table: Arc<RwLock<RoutingTable>>,
    /// We store a routing table for each peer we receive Route Update Requests from.
    /// When the peer sends us an update, we apply that update to this view of their table.
    /// Updates from peers are applied to our local_table if they are better than the
    /// existing best route and if they do not attempt to overwrite configured routes.
    incoming_tables: Arc<RwLock<HashMap<A::AccountId, RoutingTable>>>,
}

impl<S, A> CcpServerService<S, A>
where
    S: IncomingService<A>,
    A: Account + CcpAccount,
{
    // Note the next service will be used both to pass on incoming requests that are not CCP requests
    // as well as send outgoing messages for CCP
    pub fn new(ilp_address: Bytes, next: S) -> Self {
        // The global prefix is the first part of the address (for example "g." for the global address space, "example", "test", etc)
        let global_prefix: Bytes = ilp_address
            .iter()
            .position(|c| c == &b'.')
            .map(|index| ilp_address.slice_to(index + 1))
            .unwrap_or_else(|| ilp_address.clone());

        CcpServerService {
            ilp_address,
            global_prefix,
            next,
            account_type: PhantomData,
            forwarding_table: Arc::new(RwLock::new(RoutingTable::default())),
            local_table: Arc::new(RwLock::new(RoutingTable::default())),
            incoming_tables: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Handle a CCP Route Control Request. If this is from an account that we broadcast routes to,
    /// we'll send an outgoing Route Update Request to them.
    fn handle_route_control_request(
        &self,
        request: IncomingRequest<A>,
    ) -> impl Future<Item = Fulfill, Error = Reject> {
        if !request.from.should_send_routes() {
            return err(RejectBuilder {
                code: ErrorCode::F00_BAD_REQUEST,
                message: b"We are not configured to send routes to you, sorry",
                triggered_by: &self.ilp_address[..],
                data: &[],
            }
            .build());
        }

        let control = RouteControlRequest::try_from(&request.prepare);
        if control.is_err() {
            return err(RejectBuilder {
                code: ErrorCode::F00_BAD_REQUEST,
                message: b"Invalid route control request",
                triggered_by: &self.ilp_address[..],
                data: &[],
            }
            .build());
        }
        let control = control.unwrap();
        debug!(
            "Got route control request from account {}: {:?}",
            request.from.id(),
            control
        );
        ok(CCP_RESPONSE.clone())
    }

    /// Remove invalid routes before processing the Route Update Request
    fn filter_routes(&self, mut update: RouteUpdateRequest) -> RouteUpdateRequest {
        update.new_routes = update
            .new_routes
            .into_iter()
            .filter(|route| {
                if !route.prefix.starts_with(&self.global_prefix) {
                    warn!("Got route for a different global prefix: {:?}", route);
                    false
                } else if route.prefix.len() <= self.global_prefix.len() {
                    warn!("Got route broadcast for the global prefix: {:?}", route);
                    false
                } else if route.path.contains(&self.ilp_address) {
                    error!(
                        "Got route broadcast with a routing loop (path includes us): {:?}",
                        route
                    );
                    false
                } else {
                    true
                }
            })
            .collect();
        update
    }

    /// Check if this Route Update Request is valid and, if so, apply any updates it contains.
    /// If updates are applied to the Incoming Routing Table for this peer, we will
    /// then check whether those routes are better than the current best ones we have in the
    /// Local Routing Table.
    fn handle_route_update_request(
        &self,
        request: IncomingRequest<A>,
    ) -> impl Future<Item = Fulfill, Error = Reject> {
        if !request.from.should_receive_routes() {
            return err(RejectBuilder {
                code: ErrorCode::F00_BAD_REQUEST,
                message: b"Your route broadcasts are not accepted here",
                triggered_by: &self.ilp_address[..],
                data: &[],
            }
            .build());
        }

        let update = RouteUpdateRequest::try_from(&request.prepare);
        if update.is_err() {
            return err(RejectBuilder {
                code: ErrorCode::F00_BAD_REQUEST,
                message: b"Invalid route update request",
                triggered_by: &self.ilp_address[..],
                data: &[],
            }
            .build());
        }
        let update = update.unwrap();
        debug!(
            "Got route update request from account {}: {:?}",
            request.from.id(),
            update
        );

        let update = self.filter_routes(update);

        let mut incoming_tables = self.incoming_tables.write();
        if !&incoming_tables.contains_key(&request.from.id()) {
            incoming_tables.insert(
                request.from.id(),
                RoutingTable::new(update.routing_table_id),
            );
        }
        match (*incoming_tables)
            .get_mut(&request.from.id())
            .expect("Should have inserted a routing table for this account")
            .handle_update_request(update)
        {
            Ok(ref prefixes_updated) => self.update_best_routes(prefixes_updated),
            Err(message) => {
                return err(RejectBuilder {
                    code: ErrorCode::F00_BAD_REQUEST,
                    message: &message.as_bytes(),
                    data: &[],
                    triggered_by: &self.ilp_address[..],
                }
                .build());
            }
        }

        ok(CCP_RESPONSE.clone())
    }

    /// Check whether the Local Routing Table currently has the best routes for the
    /// given prefixes. This is triggered when we get an incoming Route Update Request
    /// with some new or modified routes that might be better than our existing ones.
    fn update_best_routes(&self, prefixes: &[Bytes]) {}
}

impl<S, A> IncomingService<A> for CcpServerService<S, A>
where
    S: IncomingService<A> + 'static,
    A: Account + CcpAccount + 'static,
{
    type Future = BoxedIlpFuture;

    /// Handle the IncomingRequest if it is a CCP protocol message or
    /// pass it on to the next handler if not
    fn handle_request(&mut self, request: IncomingRequest<A>) -> Self::Future {
        let destination = request.prepare.destination();
        if destination == CCP_CONTROL_DESTINATION {
            Box::new(self.handle_route_control_request(request))
        } else if destination == CCP_UPDATE_DESTINATION {
            Box::new(self.handle_route_update_request(request))
        } else {
            Box::new(self.next.handle_request(request))
        }
    }
}

#[cfg(test)]
mod helpers {
    use super::*;
    use interledger_service::incoming_service_fn;

    lazy_static! {
        pub static ref ROUTING_ACCOUNT: TestAccount = TestAccount {
            id: 1,
            send_routes: true,
            receive_routes: true,
        };
        pub static ref NON_ROUTING_ACCOUNT: TestAccount = TestAccount {
            id: 1,
            send_routes: false,
            receive_routes: false,
        };
    }

    #[derive(Clone, Debug, Copy)]
    pub struct TestAccount {
        id: u64,
        receive_routes: bool,
        send_routes: bool,
    }

    impl Account for TestAccount {
        type AccountId = u64;

        fn id(&self) -> u64 {
            self.id
        }
    }

    impl CcpAccount for TestAccount {
        fn should_receive_routes(&self) -> bool {
            self.receive_routes
        }

        fn should_send_routes(&self) -> bool {
            self.send_routes
        }
    }

    pub fn test_service(
    ) -> CcpServerService<impl IncomingService<TestAccount, Future = BoxedIlpFuture>, TestAccount>
    {
        CcpServerService::new(
            Bytes::from("example.connector"),
            incoming_service_fn(|_request| {
                Box::new(err(RejectBuilder {
                    code: ErrorCode::F02_UNREACHABLE,
                    message: b"No other handler!",
                    data: &[],
                    triggered_by: b"example.connector",
                }
                .build()))
            }),
        )
    }
}

#[cfg(test)]
mod handle_route_control_request {
    use super::helpers::*;
    use super::*;
    use crate::fixtures::*;
    use std::{
        str,
        time::{Duration, SystemTime},
    };

    #[test]
    fn handles_valid_request() {
        test_service()
            .handle_request(IncomingRequest {
                prepare: CONTROL_REQUEST.to_prepare(),
                from: *ROUTING_ACCOUNT,
            })
            .wait()
            .unwrap();
    }

    #[test]
    fn rejects_from_non_sending_account() {
        let result = test_service()
            .handle_request(IncomingRequest {
                prepare: CONTROL_REQUEST.to_prepare(),
                from: *NON_ROUTING_ACCOUNT,
            })
            .wait();
        assert!(result.is_err());
        assert_eq!(
            str::from_utf8(result.unwrap_err().message()).unwrap(),
            "We are not configured to send routes to you, sorry"
        );
    }

    #[test]
    fn rejects_invalid_packet() {
        let result = test_service()
            .handle_request(IncomingRequest {
                prepare: PrepareBuilder {
                    destination: CCP_CONTROL_DESTINATION,
                    amount: 0,
                    expires_at: SystemTime::now() + Duration::from_secs(30),
                    data: &[],
                    execution_condition: &PEER_PROTOCOL_CONDITION,
                }
                .build(),
                from: *ROUTING_ACCOUNT,
            })
            .wait();
        assert!(result.is_err());
        assert_eq!(
            str::from_utf8(result.unwrap_err().message()).unwrap(),
            "Invalid route control request"
        );
    }
}

#[cfg(test)]
mod handle_route_update_request {
    use super::helpers::*;
    use super::*;
    use crate::fixtures::*;
    use std::{
        str,
        time::{Duration, SystemTime},
    };

    #[test]
    fn handles_valid_request() {
        let mut service = test_service();
        let mut update = UPDATE_REQUEST_SIMPLE.clone();
        update.to_epoch_index = 1;
        update.from_epoch_index = 0;

        service
            .handle_request(IncomingRequest {
                prepare: update.to_prepare(),
                from: *ROUTING_ACCOUNT,
            })
            .wait()
            .unwrap();
    }

    #[test]
    fn rejects_from_non_receiving_account() {
        let result = test_service()
            .handle_request(IncomingRequest {
                prepare: UPDATE_REQUEST_SIMPLE.to_prepare(),
                from: *NON_ROUTING_ACCOUNT,
            })
            .wait();
        assert!(result.is_err());
        assert_eq!(
            str::from_utf8(result.unwrap_err().message()).unwrap(),
            "Your route broadcasts are not accepted here",
        );
    }

    #[test]
    fn rejects_invalid_packet() {
        let result = test_service()
            .handle_request(IncomingRequest {
                prepare: PrepareBuilder {
                    destination: CCP_UPDATE_DESTINATION,
                    amount: 0,
                    expires_at: SystemTime::now() + Duration::from_secs(30),
                    data: &[],
                    execution_condition: &PEER_PROTOCOL_CONDITION,
                }
                .build(),
                from: *ROUTING_ACCOUNT,
            })
            .wait();
        assert!(result.is_err());
        assert_eq!(
            str::from_utf8(result.unwrap_err().message()).unwrap(),
            "Invalid route update request"
        );
    }

    #[test]
    fn adds_table_on_first_request() {
        let mut service = test_service();
        let mut update = UPDATE_REQUEST_SIMPLE.clone();
        update.to_epoch_index = 1;
        update.from_epoch_index = 0;

        service
            .handle_request(IncomingRequest {
                prepare: update.to_prepare(),
                from: *ROUTING_ACCOUNT,
            })
            .wait()
            .unwrap();
        assert_eq!(service.incoming_tables.read().len(), 1);
    }

    #[test]
    fn filters_routes_with_other_global_prefix() {
        let service = test_service();
        let mut request = UPDATE_REQUEST_SIMPLE.clone();
        request.new_routes.push(Route {
            prefix: Bytes::from("example.valid"),
            path: Vec::new(),
            auth: [0; 32],
            props: Vec::new(),
        });
        request.new_routes.push(Route {
            prefix: Bytes::from("other.prefix"),
            path: Vec::new(),
            auth: [0; 32],
            props: Vec::new(),
        });
        let request = service.filter_routes(request);
        assert_eq!(request.new_routes.len(), 1);
        assert_eq!(request.new_routes[0].prefix, Bytes::from("example.valid"));
    }

    #[test]
    fn filters_routes_for_global_prefix() {
        let service = test_service();
        let mut request = UPDATE_REQUEST_SIMPLE.clone();
        request.new_routes.push(Route {
            prefix: Bytes::from("example.valid"),
            path: Vec::new(),
            auth: [0; 32],
            props: Vec::new(),
        });
        request.new_routes.push(Route {
            prefix: Bytes::from("example."),
            path: Vec::new(),
            auth: [0; 32],
            props: Vec::new(),
        });
        let request = service.filter_routes(request);
        assert_eq!(request.new_routes.len(), 1);
        assert_eq!(request.new_routes[0].prefix, Bytes::from("example.valid"));
    }

    #[test]
    fn filters_routing_loops() {
        let service = test_service();
        let mut request = UPDATE_REQUEST_SIMPLE.clone();
        request.new_routes.push(Route {
            prefix: Bytes::from("example.valid"),
            path: vec![
                Bytes::from("example.a"),
                service.ilp_address.clone(),
                Bytes::from("example.b"),
            ],
            auth: [0; 32],
            props: Vec::new(),
        });
        request.new_routes.push(Route {
            prefix: Bytes::from("example.valid"),
            path: Vec::new(),
            auth: [0; 32],
            props: Vec::new(),
        });
        let request = service.filter_routes(request);
        assert_eq!(request.new_routes.len(), 1);
        assert_eq!(request.new_routes[0].prefix, Bytes::from("example.valid"));
    }
}
