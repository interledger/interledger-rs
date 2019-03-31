use crate::{packet::*, routing_table::RoutingTable, RouteManagerStore, RoutingAccount};
use bytes::Bytes;
use futures::{
    future::{err, ok, Either},
    Future,
};
use hashbrown::HashMap;
use interledger_packet::*;
use interledger_service::{Account, BoxedIlpFuture, IncomingRequest, IncomingService};
use parking_lot::RwLock;
use std::{marker::PhantomData, str, sync::Arc};
use tokio_executor::spawn;

/// The Routing Manager Service.
///
/// This implements the Connector-to-Connector Protocol (CCP)
/// for exchanging route updates with peers. This service handles incoming CCP messages
/// and sends updates to peers. It manages the routing table in the Store and updates it
/// with the best routes determined by per-account configuration and the broadcasts we have
/// received from peers.
#[derive(Clone)]
pub struct CcpServerService<S, T, A: Account> {
    ilp_address: Bytes,
    global_prefix: Bytes,
    /// The next incoming request handler. This will be used both to pass on requests that are
    /// not CCP messages AND to send outgoing CCP messages to peers.
    next: S,
    account_type: PhantomData<A>,
    /// This represents the routing table we will forward to our peers.
    /// It is the same as the local_table with our own address added to the path of each route.
    forwarding_table: Arc<RwLock<RoutingTable<A>>>,
    /// This is the routing table we have compile from configuration and
    /// broadcasts we have received from our peers. It is saved to the Store so that
    /// the Router services forwards packets according to what it says.
    local_table: Arc<RwLock<RoutingTable<A>>>,
    /// We store a routing table for each peer we receive Route Update Requests from.
    /// When the peer sends us an update, we apply that update to this view of their table.
    /// Updates from peers are applied to our local_table if they are better than the
    /// existing best route and if they do not attempt to overwrite configured routes.
    incoming_tables: Arc<RwLock<HashMap<A::AccountId, RoutingTable<A>>>>,
    store: T,
    /// If true, the task to update the routing tables based on a Route Update will be
    /// spawned and run in the background. If false, the response will only be returned
    /// once the routing tables have been updated. This is primarily to help with testing
    /// because spawning doesn't work well on Tokio's current_thread executor.
    should_spawn_update: bool,
}

impl<S, T, A> CcpServerService<S, T, A>
where
    S: IncomingService<A> + Send + Sync + 'static,
    T: RouteManagerStore<Account = A> + Send + Sync + 'static,
    A: RoutingAccount + Send + Sync + 'static,
{
    // Note the next service will be used both to pass on incoming requests that are not CCP requests
    // as well as send outgoing messages for CCP
    pub fn new(ilp_address: Bytes, store: T, next: S) -> Self {
        CcpServerService::with_spawn_bool(ilp_address, store, next, true)
    }

    pub(crate) fn with_spawn_bool(
        ilp_address: Bytes,
        store: T,
        next: S,
        should_spawn_update: bool,
    ) -> Self {
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
            store,
            should_spawn_update,
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
            return Either::B(err(RejectBuilder {
                code: ErrorCode::F00_BAD_REQUEST,
                message: b"Your route broadcasts are not accepted here",
                triggered_by: &self.ilp_address[..],
                data: &[],
            }
            .build()));
        }

        let update = RouteUpdateRequest::try_from(&request.prepare);
        if update.is_err() {
            return Either::B(err(RejectBuilder {
                code: ErrorCode::F00_BAD_REQUEST,
                message: b"Invalid route update request",
                triggered_by: &self.ilp_address[..],
                data: &[],
            }
            .build()));
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
        let ilp_address = self.ilp_address.clone();
        match (*incoming_tables)
            .get_mut(&request.from.id())
            .expect("Should have inserted a routing table for this account")
            .handle_update_request(request.from.clone(), update)
        {
            Ok(prefixes_updated) => Either::A(self.maybe_spawn_update(prefixes_updated)),
            Err(message) => Either::B(err(RejectBuilder {
                code: ErrorCode::F00_BAD_REQUEST,
                message: &message.as_bytes(),
                data: &[],
                triggered_by: &ilp_address[..],
            }
            .build())),
        }
    }

    /// Either spawn the update task or wait for it to finish, depending on the configuration
    fn maybe_spawn_update(
        &self,
        prefixes_updated: Vec<Bytes>,
    ) -> impl Future<Item = Fulfill, Error = Reject> {
        // Either run the update task in the background or wait
        // until its done before we respond to the peer
        let future = if self.should_spawn_update {
            spawn(self.update_best_routes(prefixes_updated));
            Either::A(ok(()))
        } else {
            Either::B(self.update_best_routes(prefixes_updated))
        };
        let ilp_address = self.ilp_address.clone();
        future
            .map_err(move |_| {
                RejectBuilder {
                    code: ErrorCode::T00_INTERNAL_ERROR,
                    message: b"Error processing route update",
                    data: &[],
                    triggered_by: &ilp_address[..],
                }
                .build()
            })
            .and_then(|_| Ok(CCP_RESPONSE.clone()))
    }

    /// Check whether the Local Routing Table currently has the best routes for the
    /// given prefixes. This is triggered when we get an incoming Route Update Request
    /// with some new or modified routes that might be better than our existing ones.
    fn update_best_routes(
        &self,
        prefixes: Vec<Bytes>,
    ) -> impl Future<Item = (), Error = ()> + 'static {
        let local_table = self.local_table.clone();
        let forwarding_table = self.forwarding_table.clone();
        let incoming_tables = self.incoming_tables.clone();
        let ilp_address = self.ilp_address.clone();
        let mut store = self.store.clone();

        self.store.get_local_and_configured_routes().and_then(
            move |(ref local_routes, ref configured_routes)| {
                let better_routes: Vec<(Bytes, A, Route)> = {
                    // Note we only use a read lock here and later get a write lock if we need to update the table
                    let local_table = local_table.read();
                    let incoming_tables = incoming_tables.read();

                    prefixes
                        .iter()
                        .filter_map(move |prefix| {
                            // See which prefixes there is now a better route for
                            if let Some((best_next_account, best_route)) = get_best_route_for_prefix(
                                local_routes,
                                configured_routes,
                                &incoming_tables,
                                prefix.as_ref(),
                            ) {
                                if let Some((ref next_account, ref route)) = local_table.get_route(&prefix) {
                                    if next_account.id() == best_next_account.id() {
                                        None
                                    } else {
                                        Some((prefix.clone(), next_account.clone(), route.clone()))
                                    }
                                } else {
                                    Some((prefix.clone(), best_next_account, best_route))
                                }
                            } else {
                                None
                            }
                        })
                        .collect()
                };

                // Update the local and forwarding tables
                if !better_routes.is_empty() {
                    let mut local_table = local_table.write();
                    let mut forwarding_table = forwarding_table.write();

                    for (prefix, account, mut route) in better_routes {
                        debug!(
                            "Setting new route for prefix: {} -> Account {}",
                            str::from_utf8(prefix.as_ref()).unwrap_or("<not utf8>"),
                            account.id(),
                        );
                        local_table.set_route(prefix.clone(), account.clone(), route.clone());

                        // Add the same route to the forwarding table with our address added to the path
                        route.path.insert(0, ilp_address.clone());
                        forwarding_table.set_route(prefix.clone(), account.clone(), route);
                    }

                    Either::A(store.set_routes(local_table.get_simplified_table()))
                } else {
                    Either::B(ok(()))
                }

            },
        )
    }
}

fn get_best_route_for_prefix<A: RoutingAccount>(
    local_routes: &HashMap<Bytes, A>,
    configured_routes: &HashMap<Bytes, A>,
    incoming_tables: &HashMap<A::AccountId, RoutingTable<A>>,
    prefix: &[u8],
) -> Option<(A, Route)> {
    if let Some(account) = configured_routes.get(prefix) {
        return Some((
            account.clone(),
            Route {
                // TODO the Address type should let us avoid this copy
                prefix: Bytes::from(account.client_address()),
                auth: [0; 32],
                path: Vec::new(),
                props: Vec::new(),
            },
        ));
    }
    if let Some(account) = local_routes.get(prefix) {
        return Some((
            account.clone(),
            Route {
                prefix: Bytes::from(account.client_address()),
                auth: [0; 32],
                path: Vec::new(),
                props: Vec::new(),
            },
        ));
    }

    let mut candidate_routes = incoming_tables
        .values()
        .filter_map(|incoming_table| incoming_table.get_route(prefix));
    if let Some((account, route)) = candidate_routes.next() {
        let (best_account, best_route) = candidate_routes.fold(
            (account, route),
            |(best_account, best_route), (account, route)| {
                // Prioritize child > peer > parent
                if best_account.routing_relation() > account.routing_relation() {
                    return (best_account, best_route);
                } else if best_account.routing_relation() < account.routing_relation() {
                    return (account, route);
                }

                // Prioritize shortest path
                if best_route.path.len() < route.path.len() {
                    return (best_account, best_route);
                } else if best_route.path.len() > route.path.len() {
                    return (account, route);
                }

                // Finally base it on account ID
                if best_account.id().to_string() < account.id().to_string() {
                    (best_account, best_route)
                } else {
                    (account, route)
                }
            },
        );
        Some((best_account.clone(), best_route.clone()))
    } else {
        None
    }
}

impl<S, T, A> IncomingService<A> for CcpServerService<S, T, A>
where
    S: IncomingService<A> + Send + Sync + 'static,
    T: RouteManagerStore<Account = A> + Send + Sync + 'static,
    A: RoutingAccount + Send + Sync + 'static,
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
mod ranking_routes {
    use super::*;
    use crate::test_helpers::*;
    use crate::RoutingRelation;
    use std::iter::FromIterator;

    lazy_static! {
        static ref LOCAL: HashMap<Bytes, TestAccount> = HashMap::from_iter(vec![
            (
                Bytes::from("example.a"),
                TestAccount::new(1, "example.local.one")
            ),
            (
                Bytes::from("example.b"),
                TestAccount::new(2, "example.local.two")
            ),
            (
                Bytes::from("example.c"),
                TestAccount::new(3, "example.local.three")
            ),
        ]);
        static ref CONFIGURED: HashMap<Bytes, TestAccount> = HashMap::from_iter(vec![
            (
                Bytes::from("example.a"),
                TestAccount::new(4, "example.local.four")
            ),
            (
                Bytes::from("example.b"),
                TestAccount::new(5, "example.local.five")
            ),
        ]);
        static ref INCOMING: HashMap<u64, RoutingTable<TestAccount>> = {
            let mut child_table = RoutingTable::default();
            let mut child = TestAccount::new(6, "example.child");
            child.relation = RoutingRelation::Child;
            child_table.add_route(
                child.clone(),
                Route {
                    prefix: Bytes::from("example.d"),
                    path: vec![Bytes::from("example.one")],
                    auth: [0; 32],
                    props: Vec::new(),
                },
            );
            let mut peer_table_1 = RoutingTable::default();
            let peer_1 = TestAccount::new(7, "example.peer1");
            peer_table_1.add_route(
                peer_1.clone(),
                Route {
                    prefix: Bytes::from("example.d"),
                    path: Vec::new(),
                    auth: [0; 32],
                    props: Vec::new(),
                },
            );
            peer_table_1.add_route(
                peer_1.clone(),
                Route {
                    prefix: Bytes::from("example.e"),
                    path: vec![Bytes::from("example.one")],
                    auth: [0; 32],
                    props: Vec::new(),
                },
            );
            let mut peer_table_2 = RoutingTable::default();
            let peer_2 = TestAccount::new(8, "example.peer2");
            peer_table_2.add_route(
                peer_2.clone(),
                Route {
                    prefix: Bytes::from("example.e"),
                    path: vec![Bytes::from("example.one"), Bytes::from("example.two")],
                    auth: [0; 32],
                    props: Vec::new(),
                },
            );
            HashMap::from_iter(vec![(6, child_table), (7, peer_table_1), (8, peer_table_2)])
        };
    }

    #[test]
    fn prioritizes_configured_routes() {
        let best_route = get_best_route_for_prefix(&LOCAL, &CONFIGURED, &INCOMING, b"example.a");
        assert_eq!(best_route.unwrap().0.id(), 4);
    }

    #[test]
    fn prioritizes_local_routes_over_broadcasted_ones() {
        let best_route = get_best_route_for_prefix(&LOCAL, &CONFIGURED, &INCOMING, b"example.c");
        assert_eq!(best_route.unwrap().0.id(), 3);
    }

    #[test]
    fn prioritizes_children_over_peers() {
        let best_route = get_best_route_for_prefix(&LOCAL, &CONFIGURED, &INCOMING, b"example.d");
        assert_eq!(best_route.unwrap().0.id(), 6);
    }

    #[test]
    fn prioritizes_shorter_paths() {
        let best_route = get_best_route_for_prefix(&LOCAL, &CONFIGURED, &INCOMING, b"example.e");
        assert_eq!(best_route.unwrap().0.id(), 7);
    }

    #[test]
    fn returns_none_for_no_route() {
        let best_route = get_best_route_for_prefix(&LOCAL, &CONFIGURED, &INCOMING, b"example.z");
        assert!(best_route.is_none());
    }
}

#[cfg(test)]
mod handle_route_control_request {
    use super::*;
    use crate::fixtures::*;
    use crate::test_helpers::*;
    use std::time::{Duration, SystemTime};

    #[test]
    fn handles_valid_request() {
        test_service()
            .handle_request(IncomingRequest {
                prepare: CONTROL_REQUEST.to_prepare(),
                from: ROUTING_ACCOUNT.clone(),
            })
            .wait()
            .unwrap();
    }

    #[test]
    fn rejects_from_non_sending_account() {
        let result = test_service()
            .handle_request(IncomingRequest {
                prepare: CONTROL_REQUEST.to_prepare(),
                from: NON_ROUTING_ACCOUNT.clone(),
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
                from: ROUTING_ACCOUNT.clone(),
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
    use super::*;
    use crate::fixtures::*;
    use crate::test_helpers::*;
    use std::{
        iter::FromIterator,
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
                from: ROUTING_ACCOUNT.clone(),
            })
            .wait()
            .unwrap();
    }

    #[test]
    fn rejects_from_non_receiving_account() {
        let result = test_service()
            .handle_request(IncomingRequest {
                prepare: UPDATE_REQUEST_SIMPLE.to_prepare(),
                from: NON_ROUTING_ACCOUNT.clone(),
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
                from: ROUTING_ACCOUNT.clone(),
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
                from: ROUTING_ACCOUNT.clone(),
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

    #[test]
    fn updates_local_routing_table() {
        let mut service = test_service();
        let mut request = UPDATE_REQUEST_COMPLEX.clone();
        request.to_epoch_index = 1;
        request.from_epoch_index = 0;
        service
            .handle_request(IncomingRequest {
                from: ROUTING_ACCOUNT.clone(),
                prepare: request.to_prepare(),
            })
            .wait()
            .unwrap();
        assert_eq!(
            (*service.local_table.read())
                .get_route(b"example.prefix1")
                .unwrap()
                .0
                .id(),
            ROUTING_ACCOUNT.id()
        );
        assert_eq!(
            (*service.local_table.read())
                .get_route(b"example.prefix2")
                .unwrap()
                .0
                .id(),
            ROUTING_ACCOUNT.id()
        );
    }

    #[test]
    fn writes_local_routing_table_to_store() {
        let mut service = test_service();
        let mut request = UPDATE_REQUEST_COMPLEX.clone();
        request.to_epoch_index = 1;
        request.from_epoch_index = 0;
        service
            .handle_request(IncomingRequest {
                from: ROUTING_ACCOUNT.clone(),
                prepare: request.to_prepare(),
            })
            .wait()
            .unwrap();
        assert_eq!(
            service
                .store
                .routes
                .lock()
                .get(&b"example.prefix1"[..])
                .unwrap()
                .id(),
            ROUTING_ACCOUNT.id()
        );
        assert_eq!(
            service
                .store
                .routes
                .lock()
                .get(&b"example.prefix2"[..])
                .unwrap()
                .id(),
            ROUTING_ACCOUNT.id()
        );
    }

    #[test]
    fn doesnt_overwrite_configured_or_local_routes() {
        let mut service = test_service();
        let store = TestStore::with_routes(
            HashMap::from_iter(vec![(
                Bytes::from("example.prefix1"),
                TestAccount::new(9, "example.account9"),
            )]),
            HashMap::from_iter(vec![(
                Bytes::from("example.prefix2"),
                TestAccount::new(10, "example.account10"),
            )]),
        );
        service.store = store;

        let mut request = UPDATE_REQUEST_COMPLEX.clone();
        request.to_epoch_index = 1;
        request.from_epoch_index = 0;
        service
            .handle_request(IncomingRequest {
                from: ROUTING_ACCOUNT.clone(),
                prepare: request.to_prepare(),
            })
            .wait()
            .unwrap();
        assert_eq!(
            (*service.local_table.read())
                .get_route(b"example.prefix1")
                .unwrap()
                .0
                .id(),
            9
        );
        assert_eq!(
            (*service.local_table.read())
                .get_route(b"example.prefix2")
                .unwrap()
                .0
                .id(),
            10
        );
    }
}
