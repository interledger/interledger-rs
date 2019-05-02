use crate::{packet::*, routing_table::RoutingTable, CcpRoutingAccount, RouteManagerStore};
use bytes::Bytes;
use futures::{
    future::{err, join_all, ok, Either},
    Future, Stream,
};
use hashbrown::HashMap;
use interledger_packet::*;
use interledger_service::{
    Account, BoxedIlpFuture, IncomingRequest, IncomingService, OutgoingRequest, OutgoingService,
};
use parking_lot::{Mutex, RwLock};
use ring::digest::{digest, SHA256};
use std::{
    cmp::min,
    str,
    sync::Arc,
    time::{Duration, Instant},
};
use tokio_executor::spawn;
use tokio_timer::Interval;

const DEFAULT_ROUTE_EXPIRY_TIME: u32 = 45000;
const DEFAULT_BROADCAST_INTERVAL: u64 = 30000;

fn hash(preimage: &[u8; 32]) -> [u8; 32] {
    let mut out = [0; 32];
    out.copy_from_slice(digest(&SHA256, preimage).as_ref());
    out
}

type NewAndWithDrawnRoutes = (Vec<Route>, Vec<Bytes>);

/// The Routing Manager Service.
///
/// This implements the Connector-to-Connector Protocol (CCP)
/// for exchanging route updates with peers. This service handles incoming CCP messages
/// and sends updates to peers. It manages the routing table in the Store and updates it
/// with the best routes determined by per-account configuration and the broadcasts we have
/// received from peers.
#[derive(Clone)]
pub struct CcpRouteManager<S, T, U, A: Account> {
    account: A,
    ilp_address: Bytes,
    global_prefix: Bytes,
    /// The next request handler that will be used both to pass on requests that are not CCP messages.
    next_incoming: S,
    /// The outgoing request handler that will be used to send outgoing CCP messages.
    /// Note that this service bypasses the Router because the Route Manager needs to be able to
    /// send messages directly to specific peers.
    outgoing: T,
    /// This represents the routing table we will forward to our peers.
    /// It is the same as the local_table with our own address added to the path of each route.
    forwarding_table: Arc<RwLock<RoutingTable<A>>>,
    last_epoch_updates_sent_for: Arc<Mutex<u32>>,
    /// These updates are stored such that index 0 is the transition from epoch 0 to epoch 1
    forwarding_table_updates: Arc<RwLock<HashMap<u32, NewAndWithDrawnRoutes>>>,
    /// This is the routing table we have compile from configuration and
    /// broadcasts we have received from our peers. It is saved to the Store so that
    /// the Router services forwards packets according to what it says.
    local_table: Arc<RwLock<RoutingTable<A>>>,
    /// We store a routing table for each peer we receive Route Update Requests from.
    /// When the peer sends us an update, we apply that update to this view of their table.
    /// Updates from peers are applied to our local_table if they are better than the
    /// existing best route and if they do not attempt to overwrite configured routes.
    incoming_tables: Arc<RwLock<HashMap<A::AccountId, RoutingTable<A>>>>,
    store: U,
    /// If true, tasks will be spawned to process Route Update Requests and respond
    /// to Route Control Requests. If false, the response to the incoming request
    /// will wait until the outgoing messages have been sent out.
    /// Making requests wait is primarily intended for testing so that the tests do
    /// not need to be run with a proper executor like Tokio. When running this for real,
    /// it is better to respond to peer messages immediately.
    spawn_tasks: bool,
}

impl<S, T, U, A> CcpRouteManager<S, T, U, A>
where
    S: IncomingService<A> + Clone + Send + Sync + 'static,
    T: OutgoingService<A> + Clone + Send + Sync + 'static,
    U: RouteManagerStore<Account = A> + Clone + Send + Sync + 'static,
    A: CcpRoutingAccount + Send + Sync + 'static,
{
    /// Create a new Route Manager service and spawn a task to broadcast the routes
    /// to peers every 30 seconds.
    pub fn new(account: A, store: U, outgoing: T, next_incoming: S) -> Self {
        let service =
            CcpRouteManager::with_spawn_bool(account, store, outgoing, next_incoming, true);
        spawn(service.broadcast_routes(DEFAULT_BROADCAST_INTERVAL));
        service
    }

    /// Create a new Route Manager Service but don't spawn a task to broadcast routes.
    /// The `broadcast_routes` method must be called directly to start broadcasting.
    pub fn new_without_spawn_broadcast(
        account: A,
        store: U,
        outgoing: T,
        next_incoming: S,
    ) -> Self {
        CcpRouteManager::with_spawn_bool(account, store, outgoing, next_incoming, false)
    }

    pub(crate) fn with_spawn_bool(
        account: A,
        store: U,
        outgoing: T,
        next_incoming: S,
        spawn_tasks: bool,
    ) -> Self {
        // The global prefix is the first part of the address (for example "g." for the global address space, "example", "test", etc)
        let ilp_address = Bytes::from(account.client_address());
        let global_prefix: Bytes = ilp_address
            .iter()
            .position(|c| c == &b'.')
            .map(|index| ilp_address.slice_to(index + 1))
            .unwrap_or_else(|| ilp_address.clone());

        CcpRouteManager {
            account,
            ilp_address,
            global_prefix,
            next_incoming,
            outgoing,
            forwarding_table: Arc::new(RwLock::new(RoutingTable::default())),
            forwarding_table_updates: Arc::new(RwLock::new(HashMap::new())),
            last_epoch_updates_sent_for: Arc::new(Mutex::new(0)),
            local_table: Arc::new(RwLock::new(RoutingTable::default())),
            incoming_tables: Arc::new(RwLock::new(HashMap::new())),
            store,
            spawn_tasks,
        }
    }

    /// Returns a future that will trigger this service to update its routes and broadcast
    /// updates to peers on the given interval.
    pub fn broadcast_routes(&self, interval: u64) -> impl Future<Item = (), Error = ()> {
        let clone = self.clone();
        Interval::new(Instant::now(), Duration::from_millis(interval))
            .map_err(|err| error!("Interval error, no longer sending route updates: {:?}", err))
            .for_each(move |_| {
                let clone = clone.clone();
                clone
                    .clone()
                    .update_best_routes(None)
                    .and_then(move |_| clone.send_route_updates())
            })
    }

    /// Handle a CCP Route Control Request. If this is from an account that we broadcast routes to,
    /// we'll send an outgoing Route Update Request to them.
    fn handle_route_control_request(
        &self,
        request: IncomingRequest<A>,
    ) -> impl Future<Item = Fulfill, Error = Reject> {
        if !request.from.should_send_routes() {
            return Either::A(err(RejectBuilder {
                code: ErrorCode::F00_BAD_REQUEST,
                message: b"We are not configured to send routes to you, sorry",
                triggered_by: &self.ilp_address[..],
                data: &[],
            }
            .build()));
        }

        let control = RouteControlRequest::try_from(&request.prepare);
        if control.is_err() {
            return Either::A(err(RejectBuilder {
                code: ErrorCode::F00_BAD_REQUEST,
                message: b"Invalid route control request",
                triggered_by: &self.ilp_address[..],
                data: &[],
            }
            .build()));
        }
        let control = control.unwrap();
        debug!(
            "Got route control request from account {}: {:?}",
            request.from.id(),
            control
        );

        // TODO stop sending updates if they are in Idle mode
        if control.mode == Mode::Sync {
            let (from_epoch_index, to_epoch_index) = {
                let forwarding_table = self.forwarding_table.read();
                let to_epoch_index = forwarding_table.epoch();
                let from_epoch_index =
                    if control.last_known_routing_table_id != forwarding_table.id() {
                        0
                    } else {
                        min(control.last_known_epoch, to_epoch_index)
                    };
                (from_epoch_index, to_epoch_index)
            };

            if !self.spawn_tasks {
                let ilp_address = self.ilp_address.clone();
                return Either::B(
                    self.send_route_update(request.from.clone(), from_epoch_index, to_epoch_index)
                        .map_err(move |_| {
                            RejectBuilder {
                                code: ErrorCode::T01_PEER_UNREACHABLE,
                                message: b"Error sending route update request",
                                data: &[],
                                triggered_by: &ilp_address[..],
                            }
                            .build()
                        })
                        .and_then(|_| Ok(CCP_RESPONSE.clone())),
                );
            } else {
                spawn(self.send_route_update(
                    request.from.clone(),
                    from_epoch_index,
                    to_epoch_index,
                ));
            }
        }

        Either::A(ok(CCP_RESPONSE.clone()))
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
    fn handle_route_update_request(&self, request: IncomingRequest<A>) -> BoxedIlpFuture {
        if !request.from.should_receive_routes() {
            return Box::new(err(RejectBuilder {
                code: ErrorCode::F00_BAD_REQUEST,
                message: b"Your route broadcasts are not accepted here",
                triggered_by: &self.ilp_address[..],
                data: &[],
            }
            .build()));
        }

        let update = RouteUpdateRequest::try_from(&request.prepare);
        if update.is_err() {
            return Box::new(err(RejectBuilder {
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
            Ok(prefixes_updated) => {
                let future = self.update_best_routes(Some(prefixes_updated));
                if self.spawn_tasks {
                    spawn(future);
                    Box::new(ok(CCP_RESPONSE.clone()))
                } else {
                    let ilp_address = self.ilp_address.clone();
                    Box::new(
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
                            .and_then(|_| Ok(CCP_RESPONSE.clone())),
                    )
                }
            }
            Err(message) => {
                let reject = RejectBuilder {
                    code: ErrorCode::F00_BAD_REQUEST,
                    message: &message.as_bytes(),
                    data: &[],
                    triggered_by: &ilp_address[..],
                }
                .build();
                let table = &incoming_tables[&request.from.id()];
                let future = self.send_route_control_request(
                    request.from.clone(),
                    table.id(),
                    table.epoch(),
                );
                if self.spawn_tasks {
                    spawn(future);
                    Box::new(err(reject))
                } else {
                    Box::new(future.then(move |_| Err(reject)))
                }
            }
        }
    }

    /// Request a Route Update from the specified peer. This is sent when we get
    /// a Route Update Request from them with a gap in the epochs since the last one we saw.
    fn send_route_control_request(
        &self,
        to: A,
        last_known_routing_table_id: [u8; 16],
        last_known_epoch: u32,
    ) -> impl Future<Item = (), Error = ()> {
        let to_id = to.id();
        let control = RouteControlRequest {
            mode: Mode::Sync,
            last_known_routing_table_id,
            last_known_epoch,
            features: Vec::new(),
        };
        debug!("Sending Route Control Request to account: {}, last known table id: {}, last known epoch: {}", to.id(), hex::encode(&last_known_routing_table_id[..]), last_known_epoch);
        let prepare = control.to_prepare();
        self.clone()
            .outgoing
            .send_request(OutgoingRequest {
                from: self.account.clone(),
                to,
                original_amount: prepare.amount(),
                prepare,
            })
            .then(move |result| {
                if let Err(err) = result {
                    error!(
                        "Error sending Route Control Request to account {}: {:?}",
                        to_id, err
                    )
                } else {
                    trace!("Sent Route Control Request to account: {}", to_id);
                }
                Ok(())
            })
    }

    /// Check whether the Local Routing Table currently has the best routes for the
    /// given prefixes. This is triggered when we get an incoming Route Update Request
    /// with some new or modified routes that might be better than our existing ones.
    ///
    /// If prefixes is None, this will check the best routes for all local and configured prefixes.
    fn update_best_routes(
        &self,
        prefixes: Option<Vec<Bytes>>,
    ) -> impl Future<Item = (), Error = ()> + 'static {
        let local_table = self.local_table.clone();
        let forwarding_table = self.forwarding_table.clone();
        let forwarding_table_updates = self.forwarding_table_updates.clone();
        let incoming_tables = self.incoming_tables.clone();
        let ilp_address = self.ilp_address.clone();
        let global_prefix = self.global_prefix.clone();
        let mut store = self.store.clone();

        self.store.get_local_and_configured_routes().and_then(
            move |(ref local_routes, ref configured_routes)| {
                let (better_routes, withdrawn_routes) = {
                    // Note we only use a read lock here and later get a write lock if we need to update the table
                    let local_table = local_table.read();
                    let incoming_tables = incoming_tables.read();

                    // Either check the given prefixes or check all of our local and configured routes
                    let prefixes_to_check: Box<Iterator<Item = Bytes>> = if let Some(prefixes) = prefixes {
                        Box::new(prefixes.into_iter())
                    } else {
                        let routes = configured_routes.iter().chain(local_routes.iter());
                        Box::new(routes.map(|(prefix, _account)| prefix.clone()))
                    };

                    // Check all the prefixes to see which ones we have different routes for
                    // and which ones we don't have routes for anymore
                    let mut better_routes: Vec<(Bytes, A, Route)> = Vec::with_capacity(prefixes_to_check.size_hint().0);
                    let mut withdrawn_routes: Vec<Bytes> = Vec::new();
                    for prefix in prefixes_to_check {
                        // See which prefixes there is now a better route for
                        if let Some((best_next_account, best_route)) = get_best_route_for_prefix(
                            local_routes,
                            configured_routes,
                            &incoming_tables,
                            prefix.as_ref(),
                        ) {
                            if let Some((ref next_account, ref route)) = local_table.get_route(&prefix) {
                                if next_account.id() == best_next_account.id() {
                                    continue
                                } else {
                                    better_routes.push((prefix.clone(), next_account.clone(), route.clone()));
                                }
                            } else {
                                better_routes.push((prefix.clone(), best_next_account, best_route));
                            }
                        } else {
                            // No longer have a route to this prefix
                            withdrawn_routes.push(prefix);
                        }
                    }
                    (better_routes, withdrawn_routes)
                };

                // Update the local and forwarding tables
                if !better_routes.is_empty() || !withdrawn_routes.is_empty() {
                    let mut local_table = local_table.write();
                    let mut forwarding_table = forwarding_table.write();
                    let mut forwarding_table_updates = forwarding_table_updates.write();

                    let mut new_routes: Vec<Route> = Vec::with_capacity(better_routes.len());

                    for (prefix, account, mut route) in better_routes {
                        debug!(
                            "Setting new route for prefix: {} -> Account {}",
                            str::from_utf8(prefix.as_ref()).unwrap_or("<not utf8>"),
                            account.id(),
                        );
                        local_table.set_route(prefix.clone(), account.clone(), route.clone());

                        // Update the forwarding table
                        // Don't advertise routes that don't start with the global prefix
                        if route.prefix.starts_with(&global_prefix[..])
                            // Don't advertise the global prefix
                            && route.prefix != global_prefix
                            // Don't advertise completely local routes because advertising our own
                            // prefix will make sure we get packets sent to them
                            && !(route.prefix.starts_with(&ilp_address[..]) && route.path.is_empty())
                            // Don't include routes we're also withdrawing
                            && !withdrawn_routes.contains(&prefix) {

                                let old_route = forwarding_table.get_route(&prefix);
                                if old_route.is_none() || old_route.unwrap().0.id() != account.id() {
                                    route.path.insert(0, ilp_address.clone());
                                    // Each hop hashes the auth before forwarding
                                    route.auth = hash(&route.auth);
                                    forwarding_table.set_route(prefix.clone(), account.clone(), route.clone());
                                    new_routes.push(route);
                                }
                        }
                    }

                    for prefix in withdrawn_routes.iter() {
                        debug!("Removed route for prefix: {}", str::from_utf8(&prefix[..]).unwrap_or("<not utf8>"));
                        local_table.delete_route(prefix);
                        forwarding_table.delete_route(prefix);
                    }

                    let epoch = forwarding_table.increment_epoch();
                    forwarding_table_updates.insert(epoch, (new_routes, withdrawn_routes));

                    Either::A(store.set_routes(local_table.get_simplified_table()))
                } else {
                    // The routing table hasn't changed
                    Either::B(ok(()))
                }
            },
        )
    }

    /// Send RouteUpdateRequests to all peers that we send routing messages to
    fn send_route_updates(&self) -> impl Future<Item = (), Error = ()> {
        let mut outgoing = self.outgoing.clone();
        let account = self.account.clone();
        let to_epoch_index = self.forwarding_table.read().epoch();

        let from_epoch_index: u32 = {
            let mut lock = self.last_epoch_updates_sent_for.lock();
            let epoch = *lock;
            *lock = to_epoch_index;
            epoch
        };

        trace!(
            "Sending route udpates for epochs: {} - {}",
            from_epoch_index,
            to_epoch_index
        );

        let prepare = self
            .create_route_update(from_epoch_index, to_epoch_index)
            .to_prepare();
        self.store
            .get_accounts_to_send_routes_to()
            .and_then(move |mut accounts| {
                accounts.sort_unstable_by_key(|a| a.id().to_string());
                accounts.dedup_by_key(|a| a.id());

                let broadcasting = !accounts.is_empty();
                if broadcasting {
                    let account_list: Vec<String> =
                        accounts.iter().map(|a| a.id().to_string()).collect();
                    trace!(
                        "Sending route updates to accounts: {}",
                        account_list.join(", ")
                    );
                    Either::A(
                        join_all(accounts.into_iter().map(move |to| {
                            let to_id = to.id();
                            outgoing
                                .send_request(OutgoingRequest {
                                    from: account.clone(),
                                    to,
                                    original_amount: prepare.amount(),
                                    prepare: prepare.clone(),
                                })
                                .map_err(move |err| {
                                    error!(
                                        "Error sending route update to account {}: {:?}",
                                        to_id, err
                                    )
                                })
                                .and_then(|_| Ok(()))
                        }))
                        .and_then(|_| {
                            trace!("Finished sending route updates");
                            Ok(())
                        }),
                    )
                } else {
                    trace!("No accounts to broadcast routes to");
                    Either::B(ok(()))
                }
            })
    }

    /// Create a RouteUpdateRequest representing the given range of Forwarding Routing Table epochs.
    /// If the epoch range is not specified, it will create an update for the last epoch only.
    fn create_route_update(
        &self,
        from_epoch_index: u32,
        to_epoch_index: u32,
    ) -> RouteUpdateRequest {
        let (routing_table_id, current_epoch_index) = {
            let table = self.forwarding_table.read();
            (table.id(), table.epoch())
        };
        let forwarding_table_updates = self.forwarding_table_updates.read();
        let epochs_to_take: usize = if to_epoch_index > from_epoch_index {
            (to_epoch_index - from_epoch_index) as usize
        } else {
            0
        };

        // Merge the new routes and withdrawn routes from all of the given epochs
        let mut new_routes: Vec<Route> = Vec::with_capacity(epochs_to_take);
        let mut withdrawn_routes: Vec<Bytes> = Vec::new();
        // Iterate through each of the given epochs
        for (new, withdrawn) in forwarding_table_updates
            .values()
            .skip(from_epoch_index as usize)
            .take(epochs_to_take)
        {
            for new_route in new {
                new_routes.push(new_route.clone());
                // If the route was previously withdrawn, ignore that now since it was added back
                if withdrawn_routes.contains(&new_route.prefix) {
                    withdrawn_routes = withdrawn_routes
                        .into_iter()
                        .filter(|prefix| prefix != &new_route.prefix)
                        .collect();
                }
            }

            for withdrawn_route in withdrawn {
                withdrawn_routes.push(withdrawn_route.clone());
                // If the route was previously added, ignore that since it was withdrawn later
                if new_routes
                    .iter()
                    .any(|route| route.prefix == withdrawn_route)
                {
                    new_routes = new_routes
                        .into_iter()
                        .filter(|route| route.prefix != withdrawn_route)
                        .collect();
                }
            }
        }

        RouteUpdateRequest {
            routing_table_id,
            from_epoch_index,
            to_epoch_index,
            current_epoch_index,
            new_routes: new_routes.clone(),
            withdrawn_routes: withdrawn_routes.clone(),
            speaker: self.ilp_address.clone(),
            hold_down_time: DEFAULT_ROUTE_EXPIRY_TIME,
        }
    }

    /// Send a Route Update Request to a specific account for the given epoch range.
    /// This is used when the peer has fallen behind and has requested a specific range of updates.
    fn send_route_update(
        &self,
        to: A,
        from_epoch_index: u32,
        to_epoch_index: u32,
    ) -> impl Future<Item = (), Error = ()> {
        let prepare = self
            .create_route_update(from_epoch_index, to_epoch_index)
            .to_prepare();
        let to_id = to.id();
        debug!(
            "Sending individual route update to account: {} for epochs from: {} to: {}",
            to.id(),
            from_epoch_index,
            to_epoch_index
        );
        self.outgoing
            .clone()
            .send_request(OutgoingRequest {
                to,
                from: self.account.clone(),
                original_amount: prepare.amount(),
                prepare,
            })
            .and_then(|_| Ok(()))
            .then(move |result| {
                if let Err(err) = result {
                    error!("Error sending route update to account {}: {:?}", to_id, err)
                }
                Ok(())
            })
    }
}

fn get_best_route_for_prefix<A: CcpRoutingAccount>(
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

impl<S, T, U, A> IncomingService<A> for CcpRouteManager<S, T, U, A>
where
    S: IncomingService<A> + Clone + Send + Sync + 'static,
    T: OutgoingService<A> + Clone + Send + Sync + 'static,
    U: RouteManagerStore<Account = A> + Clone + Send + Sync + 'static,
    A: CcpRoutingAccount + Send + Sync + 'static,
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
            Box::new(self.next_incoming.handle_request(request))
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
        test_service_with_routes()
            .0
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

    #[test]
    fn sends_update_in_response() {
        let (mut service, outgoing_requests) = test_service_with_routes();
        (*service.forwarding_table.write()).set_id([0; 16]);
        service.update_best_routes(None).wait().unwrap();
        service
            .handle_request(IncomingRequest {
                from: ROUTING_ACCOUNT.clone(),
                prepare: RouteControlRequest {
                    last_known_routing_table_id: [0; 16],
                    mode: Mode::Sync,
                    last_known_epoch: 0,
                    features: Vec::new(),
                }
                .to_prepare(),
            })
            .wait()
            .unwrap();
        let request: &OutgoingRequest<TestAccount> = &outgoing_requests.lock()[0];
        assert_eq!(request.to.id(), ROUTING_ACCOUNT.id());
        let update = RouteUpdateRequest::try_from(&request.prepare).unwrap();
        assert_eq!(update.routing_table_id, [0; 16]);
        assert_eq!(update.from_epoch_index, 0);
        assert_eq!(update.to_epoch_index, 1);
        assert_eq!(update.current_epoch_index, 1);
        assert_eq!(update.new_routes.len(), 2);
    }

    #[test]
    fn sends_whole_table_if_id_is_different() {
        let (mut service, outgoing_requests) = test_service_with_routes();
        service.update_best_routes(None).wait().unwrap();
        service
            .handle_request(IncomingRequest {
                from: ROUTING_ACCOUNT.clone(),
                prepare: RouteControlRequest {
                    last_known_routing_table_id: [0; 16],
                    mode: Mode::Sync,
                    last_known_epoch: 32,
                    features: Vec::new(),
                }
                .to_prepare(),
            })
            .wait()
            .unwrap();
        let routing_table_id = service.forwarding_table.read().id();
        let request: &OutgoingRequest<TestAccount> = &outgoing_requests.lock()[0];
        assert_eq!(request.to.id(), ROUTING_ACCOUNT.id());
        let update = RouteUpdateRequest::try_from(&request.prepare).unwrap();
        assert_eq!(update.routing_table_id, routing_table_id);
        assert_eq!(update.from_epoch_index, 0);
        assert_eq!(update.to_epoch_index, 1);
        assert_eq!(update.current_epoch_index, 1);
        assert_eq!(update.new_routes.len(), 2);
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

    #[test]
    fn removes_withdrawn_routes() {
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
        service
            .handle_request(IncomingRequest {
                from: ROUTING_ACCOUNT.clone(),
                prepare: RouteUpdateRequest {
                    routing_table_id: UPDATE_REQUEST_COMPLEX.routing_table_id,
                    from_epoch_index: 1,
                    to_epoch_index: 3,
                    current_epoch_index: 3,
                    hold_down_time: 45000,
                    speaker: UPDATE_REQUEST_COMPLEX.speaker.clone(),
                    new_routes: Vec::new(),
                    withdrawn_routes: vec![Bytes::from("example.prefix2")],
                }
                .to_prepare(),
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
        assert!((*service.local_table.read())
            .get_route(b"example.prefix2")
            .is_none());
    }

    #[test]
    fn sends_control_request_if_routing_table_id_changed() {
        let (mut service, outgoing_requests) = test_service_with_routes();
        // First request is valid
        let mut request1 = UPDATE_REQUEST_COMPLEX.clone();
        request1.to_epoch_index = 3;
        request1.from_epoch_index = 0;
        service
            .handle_request(IncomingRequest {
                from: ROUTING_ACCOUNT.clone(),
                prepare: request1.to_prepare(),
            })
            .wait()
            .unwrap();

        // Second has a gap in epochs
        let mut request2 = UPDATE_REQUEST_COMPLEX.clone();
        request2.to_epoch_index = 8;
        request2.from_epoch_index = 7;
        request2.routing_table_id = [9; 16];
        let err = service
            .handle_request(IncomingRequest {
                from: ROUTING_ACCOUNT.clone(),
                prepare: request2.to_prepare(),
            })
            .wait()
            .unwrap_err();
        assert_eq!(err.code(), ErrorCode::F00_BAD_REQUEST);

        let request = &outgoing_requests.lock()[0];
        let control = RouteControlRequest::try_from(&request.prepare).unwrap();
        assert_eq!(control.last_known_epoch, 0);
        assert_eq!(
            control.last_known_routing_table_id,
            request2.routing_table_id
        );
    }

    #[test]
    fn sends_control_request_if_missing_epochs() {
        let (mut service, outgoing_requests) = test_service_with_routes();

        // First request is valid
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

        // Second has a gap in epochs
        let mut request = UPDATE_REQUEST_COMPLEX.clone();
        request.to_epoch_index = 8;
        request.from_epoch_index = 7;
        let err = service
            .handle_request(IncomingRequest {
                from: ROUTING_ACCOUNT.clone(),
                prepare: request.to_prepare(),
            })
            .wait()
            .unwrap_err();
        assert_eq!(err.code(), ErrorCode::F00_BAD_REQUEST);

        let request = &outgoing_requests.lock()[0];
        let control = RouteControlRequest::try_from(&request.prepare).unwrap();
        assert_eq!(control.last_known_epoch, 1);
    }
}

#[cfg(test)]
mod create_route_update {
    use super::*;
    use crate::test_helpers::*;
    use std::iter::FromIterator;

    #[test]
    fn heartbeat_message_for_empty_table() {
        let service = test_service();
        let update = service.create_route_update(0, 0);
        assert_eq!(update.from_epoch_index, 0);
        assert_eq!(update.to_epoch_index, 0);
        assert_eq!(update.current_epoch_index, 0);
        assert!(update.new_routes.is_empty());
        assert!(update.withdrawn_routes.is_empty());
    }

    #[test]
    fn includes_the_given_range_of_epochs() {
        let service = test_service();
        (*service.forwarding_table.write()).set_epoch(4);
        *service.forwarding_table_updates.write() = HashMap::from_iter(vec![
            (
                0,
                (
                    vec![Route {
                        prefix: Bytes::from("example.a"),
                        path: vec![Bytes::from("example.x")],
                        auth: [1; 32],
                        props: Vec::new(),
                    }],
                    Vec::new(),
                ),
            ),
            (
                1,
                (
                    vec![Route {
                        prefix: Bytes::from("example.b"),
                        path: vec![Bytes::from("example.x")],
                        auth: [2; 32],
                        props: Vec::new(),
                    }],
                    Vec::new(),
                ),
            ),
            (
                2,
                (
                    vec![Route {
                        prefix: Bytes::from("example.c"),
                        path: vec![Bytes::from("example.x"), Bytes::from("example.y")],
                        auth: [3; 32],
                        props: Vec::new(),
                    }],
                    vec![Bytes::from("example.m")],
                ),
            ),
            (
                3,
                (
                    vec![Route {
                        prefix: Bytes::from("example.d"),
                        path: vec![Bytes::from("example.x"), Bytes::from("example.y")],
                        auth: [4; 32],
                        props: Vec::new(),
                    }],
                    vec![Bytes::from("example.n")],
                ),
            ),
        ]);
        let update = service.create_route_update(1, 3);
        assert_eq!(update.from_epoch_index, 1);
        assert_eq!(update.to_epoch_index, 3);
        assert_eq!(update.current_epoch_index, 4);
        assert_eq!(update.new_routes.len(), 2);
        assert_eq!(update.withdrawn_routes.len(), 1);
        let new_routes: Vec<&str> = update
            .new_routes
            .iter()
            .map(|r| str::from_utf8(r.prefix.as_ref()).unwrap())
            .collect();
        assert!(new_routes.contains(&"example.b"));
        assert!(new_routes.contains(&"example.c"));
        assert!(!new_routes.contains(&"example.m"));
        assert_eq!(update.withdrawn_routes[0], &Bytes::from("example.m"));
    }
}

#[cfg(test)]
mod send_route_updates {
    use super::*;
    use crate::test_helpers::*;

    #[test]
    fn broadcasts_to_all_accounts_we_send_updates_to() {
        let (service, outgoing_requests) = test_service_with_routes();
        service.send_route_updates().wait().unwrap();
        let mut accounts: Vec<u64> = outgoing_requests
            .lock()
            .iter()
            .map(|request| request.to.id())
            .collect();
        accounts.sort_unstable();
        assert_eq!(accounts, vec![1, 2]);
    }

    #[test]
    fn broadcasts_configured_and_local_routes() {
        let (service, outgoing_requests) = test_service_with_routes();

        // This is normally spawned as a task when the service is created
        service.update_best_routes(None).wait().unwrap();

        service.send_route_updates().wait().unwrap();
        let update = RouteUpdateRequest::try_from(&outgoing_requests.lock()[0].prepare).unwrap();
        assert_eq!(update.new_routes.len(), 2);
        let prefixes: Vec<&str> = update
            .new_routes
            .iter()
            .map(|route| str::from_utf8(route.prefix.as_ref()).unwrap())
            .collect();
        assert!(prefixes.contains(&"example.local.1"));
        assert!(prefixes.contains(&"example.configured.1"));
    }

    #[test]
    fn broadcasts_received_routes() {
        let (service, outgoing_requests) = test_service_with_routes();

        // This is normally spawned as a task when the service is created
        service.update_best_routes(None).wait().unwrap();

        service
            .handle_route_update_request(IncomingRequest {
                from: TestAccount::new(10, "example.peer"),
                prepare: RouteUpdateRequest {
                    routing_table_id: [0; 16],
                    current_epoch_index: 1,
                    from_epoch_index: 0,
                    to_epoch_index: 1,
                    hold_down_time: 30000,
                    speaker: Bytes::from("example.remote"),
                    new_routes: vec![Route {
                        prefix: Bytes::from("example.remote"),
                        path: vec![Bytes::from("example.peer")],
                        auth: [0; 32],
                        props: Vec::new(),
                    }],
                    withdrawn_routes: Vec::new(),
                }
                .to_prepare(),
            })
            .wait()
            .unwrap();

        service.send_route_updates().wait().unwrap();
        let update = RouteUpdateRequest::try_from(&outgoing_requests.lock()[0].prepare).unwrap();
        assert_eq!(update.new_routes.len(), 3);
        let prefixes: Vec<&str> = update
            .new_routes
            .iter()
            .map(|route| str::from_utf8(route.prefix.as_ref()).unwrap())
            .collect();
        assert!(prefixes.contains(&"example.local.1"));
        assert!(prefixes.contains(&"example.configured.1"));
        assert!(prefixes.contains(&"example.remote"));
    }

    #[test]
    fn broadcasts_withdrawn_routes() {
        let (service, outgoing_requests) = test_service_with_routes();

        // This is normally spawned as a task when the service is created
        service.update_best_routes(None).wait().unwrap();

        service
            .handle_route_update_request(IncomingRequest {
                from: TestAccount::new(10, "example.peer"),
                prepare: RouteUpdateRequest {
                    routing_table_id: [0; 16],
                    current_epoch_index: 1,
                    from_epoch_index: 0,
                    to_epoch_index: 1,
                    hold_down_time: 30000,
                    speaker: Bytes::from("example.remote"),
                    new_routes: vec![Route {
                        prefix: Bytes::from("example.remote"),
                        path: vec![Bytes::from("example.peer")],
                        auth: [0; 32],
                        props: Vec::new(),
                    }],
                    withdrawn_routes: Vec::new(),
                }
                .to_prepare(),
            })
            .wait()
            .unwrap();
        service
            .handle_route_update_request(IncomingRequest {
                from: TestAccount::new(10, "example.peer"),
                prepare: RouteUpdateRequest {
                    routing_table_id: [0; 16],
                    current_epoch_index: 4,
                    from_epoch_index: 1,
                    to_epoch_index: 4,
                    hold_down_time: 30000,
                    speaker: Bytes::from("example.remote"),
                    new_routes: Vec::new(),
                    withdrawn_routes: vec![Bytes::from("example.remote")],
                }
                .to_prepare(),
            })
            .wait()
            .unwrap();

        service.send_route_updates().wait().unwrap();
        let update = RouteUpdateRequest::try_from(&outgoing_requests.lock()[0].prepare).unwrap();
        assert_eq!(update.new_routes.len(), 2);
        let prefixes: Vec<&str> = update
            .new_routes
            .iter()
            .map(|route| str::from_utf8(route.prefix.as_ref()).unwrap())
            .collect();
        assert!(prefixes.contains(&"example.local.1"));
        assert!(prefixes.contains(&"example.configured.1"));
        assert!(!prefixes.contains(&"example.remote"));
        assert_eq!(update.withdrawn_routes.len(), 1);
        assert_eq!(
            str::from_utf8(&update.withdrawn_routes[0]).unwrap(),
            "example.remote"
        );
    }
}
