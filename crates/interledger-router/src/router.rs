use super::RouterStore;
use bytes::Bytes;
use futures::{future::err, Future};
use interledger_packet::{ErrorCode, RejectBuilder};
use interledger_service::*;
use std::str;

/// # Interledger Router
///
/// The `Router` implements an incoming service and includes an outgoing service.
/// It determines the next account to forward to and passes it on.
/// Both incoming and outgoing services can respond to requests but many just pass the request on.
/// The `Router` requires a `RouterStore`, which keeps track of the entire routing table. Once the `Router` receives a Prepare, it checks its destination and if it finds it in the routing table
///
/// The router implements the IncomingService trait and uses the routing table
/// to determine the `to` (or "next hop") Account for the given request.
///
/// Note that the router does **not**:
///   - apply exchange rates or fees to the Prepare packet
///   - adjust account balances
///   - reduce the Prepare packet's expiry
///
/// That is done by OutgoingServices.

#[derive(Clone)]
pub struct Router<S, O> {
    store: S,
    next: O,
}

impl<S, O> Router<S, O>
where
    S: RouterStore,
    O: OutgoingService<S::Account>,
{
    pub fn new(store: S, next: O) -> Self {
        Router { store, next }
    }
}

impl<S, O> IncomingService<S::Account> for Router<S, O>
where
    S: RouterStore,
    O: OutgoingService<S::Account> + Clone + Send + 'static,
{
    type Future = BoxedIlpFuture;

    /// Figures out the next node to pass the received Prepare packet to.
    ///
    /// Firstly, it checks if there is a direct path for that account and use that.
    /// If not it scans through the routing table and checks if the route prefix matches
    /// the prepare packet's destination or if it's a catch-all address (i.e. empty prefix)
    fn handle_request(&mut self, request: IncomingRequest<S::Account>) -> Self::Future {
        let destination = request.prepare.destination();
        let mut next_hop = None;
        let routing_table = self.store.routing_table();

        // Check if we have a direct path for that account or if we need to scan
        // through the routing table
        let dest: &[u8] = destination.as_ref();
        if let Some(account_id) = routing_table.get(dest) {
            trace!(
                "Found direct route for address: \"{}\". Account: {}",
                destination,
                account_id
            );
            next_hop = Some(*account_id);
        } else if !routing_table.is_empty() {
            let mut matching_prefix = Bytes::new();
            for route in self.store.routing_table() {
                trace!(
                    "Checking route: \"{}\" -> {}",
                    str::from_utf8(&route.0[..]).unwrap_or("<not utf8>"),
                    route.1
                );
                // Check if the route prefix matches or is empty (meaning it's a catch-all address)
                if (route.0.is_empty() || dest.starts_with(&route.0[..]))
                    && route.0.len() >= matching_prefix.len()
                {
                    next_hop.replace(route.1);
                    matching_prefix = route.0;
                }
            }
            if let Some(account_id) = next_hop {
                trace!(
                    "Found matching route for address: \"{}\". Prefix: \"{}\", account: {}",
                    destination,
                    str::from_utf8(&matching_prefix[..]).unwrap_or("<not utf8>"),
                    account_id,
                );
            }
        } else {
            error!("Unable to route request because routing table is empty");
        }

        if let Some(account_id) = next_hop {
            let mut next = self.next.clone();
            Box::new(
                self.store
                    .get_accounts(vec![account_id])
                    .map_err(move |_| {
                        error!("No record found for account: {}", account_id);
                        RejectBuilder {
                            code: ErrorCode::F02_UNREACHABLE,
                            message: &[],
                            triggered_by: None,
                            data: &[],
                        }
                        .build()
                    })
                    .and_then(move |mut accounts| {
                        let request = request.into_outgoing(accounts.remove(0));
                        next.send_request(request)
                    }),
            )
        } else {
            error!("No route found for request: {:?}", request);
            Box::new(err(RejectBuilder {
                code: ErrorCode::F02_UNREACHABLE,
                message: &[],
                triggered_by: None,
                data: &[],
            }
            .build()))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::future::ok;
    use hashbrown::HashMap;
    use interledger_packet::{Address, FulfillBuilder, PrepareBuilder};
    use interledger_service::outgoing_service_fn;
    use parking_lot::Mutex;
    use std::iter::FromIterator;
    use std::str::FromStr;
    use std::sync::Arc;
    use std::time::UNIX_EPOCH;

    #[derive(Debug, Clone)]
    struct TestAccount(u64);

    impl Account for TestAccount {
        type AccountId = u64;
        fn id(&self) -> u64 {
            self.0
        }
    }

    #[derive(Clone)]
    struct TestStore {
        routes: HashMap<Bytes, u64>,
    }

    impl AccountStore for TestStore {
        type Account = TestAccount;

        fn get_accounts(
            &self,
            account_ids: Vec<<<Self as AccountStore>::Account as Account>::AccountId>,
        ) -> Box<dyn Future<Item = Vec<TestAccount>, Error = ()> + Send> {
            Box::new(ok(account_ids.into_iter().map(TestAccount).collect()))
        }
    }

    impl RouterStore for TestStore {
        fn routing_table(&self) -> HashMap<Bytes, u64> {
            self.routes.clone()
        }
    }

    #[test]
    fn empty_routing_table() {
        let mut router = Router::new(
            TestStore {
                routes: HashMap::new(),
            },
            outgoing_service_fn(|_| {
                Ok(FulfillBuilder {
                    fulfillment: &[0; 32],
                    data: &[],
                }
                .build())
            }),
        );

        let result = router
            .handle_request(IncomingRequest {
                from: TestAccount(0),
                prepare: PrepareBuilder {
                    destination: Address::from_str("example.destination").unwrap(),
                    amount: 100,
                    execution_condition: &[1; 32],
                    expires_at: UNIX_EPOCH,
                    data: &[],
                }
                .build(),
            })
            .wait();
        assert!(result.is_err());
    }

    #[test]
    fn no_route() {
        let mut router = Router::new(
            TestStore {
                routes: HashMap::from_iter(vec![(Bytes::from("example.other"), 1)].into_iter()),
            },
            outgoing_service_fn(|_| {
                Ok(FulfillBuilder {
                    fulfillment: &[0; 32],
                    data: &[],
                }
                .build())
            }),
        );

        let result = router
            .handle_request(IncomingRequest {
                from: TestAccount(0),
                prepare: PrepareBuilder {
                    destination: Address::from_str("example.destination").unwrap(),
                    amount: 100,
                    execution_condition: &[1; 32],
                    expires_at: UNIX_EPOCH,
                    data: &[],
                }
                .build(),
            })
            .wait();
        assert!(result.is_err());
    }

    #[test]
    fn finds_exact_route() {
        let mut router = Router::new(
            TestStore {
                routes: HashMap::from_iter(
                    vec![(Bytes::from("example.destination"), 1)].into_iter(),
                ),
            },
            outgoing_service_fn(|_| {
                Ok(FulfillBuilder {
                    fulfillment: &[0; 32],
                    data: &[],
                }
                .build())
            }),
        );

        let result = router
            .handle_request(IncomingRequest {
                from: TestAccount(0),
                prepare: PrepareBuilder {
                    destination: Address::from_str("example.destination").unwrap(),
                    amount: 100,
                    execution_condition: &[1; 32],
                    expires_at: UNIX_EPOCH,
                    data: &[],
                }
                .build(),
            })
            .wait();
        assert!(result.is_ok());
    }

    #[test]
    fn catch_all_route() {
        let mut router = Router::new(
            TestStore {
                routes: HashMap::from_iter(vec![(Bytes::from(""), 0)].into_iter()),
            },
            outgoing_service_fn(|_| {
                Ok(FulfillBuilder {
                    fulfillment: &[0; 32],
                    data: &[],
                }
                .build())
            }),
        );

        let result = router
            .handle_request(IncomingRequest {
                from: TestAccount(0),
                prepare: PrepareBuilder {
                    destination: Address::from_str("example.destination").unwrap(),
                    amount: 100,
                    execution_condition: &[1; 32],
                    expires_at: UNIX_EPOCH,
                    data: &[],
                }
                .build(),
            })
            .wait();
        assert!(result.is_ok());
    }

    #[test]
    fn finds_matching_prefix() {
        let mut router = Router::new(
            TestStore {
                routes: HashMap::from_iter(vec![(Bytes::from("example."), 1)].into_iter()),
            },
            outgoing_service_fn(|_| {
                Ok(FulfillBuilder {
                    fulfillment: &[0; 32],
                    data: &[],
                }
                .build())
            }),
        );

        let result = router
            .handle_request(IncomingRequest {
                from: TestAccount(0),
                prepare: PrepareBuilder {
                    destination: Address::from_str("example.destination").unwrap(),
                    amount: 100,
                    execution_condition: &[1; 32],
                    expires_at: UNIX_EPOCH,
                    data: &[],
                }
                .build(),
            })
            .wait();
        assert!(result.is_ok());
    }

    #[test]
    fn finds_longest_matching_prefix() {
        let to: Arc<Mutex<Option<TestAccount>>> = Arc::new(Mutex::new(None));
        let to_clone = to.clone();
        let mut router = Router::new(
            TestStore {
                routes: HashMap::from_iter(
                    vec![
                        (Bytes::from(""), 0),
                        (Bytes::from("example.destination"), 2),
                        (Bytes::from("example."), 1),
                    ]
                    .into_iter(),
                ),
            },
            outgoing_service_fn(move |request: OutgoingRequest<TestAccount>| {
                *to_clone.lock() = Some(request.to.clone());

                Ok(FulfillBuilder {
                    fulfillment: &[0; 32],
                    data: &[],
                }
                .build())
            }),
        );

        let result = router
            .handle_request(IncomingRequest {
                from: TestAccount(0),
                prepare: PrepareBuilder {
                    destination: Address::from_str("example.destination").unwrap(),
                    amount: 100,
                    execution_condition: &[1; 32],
                    expires_at: UNIX_EPOCH,
                    data: &[],
                }
                .build(),
            })
            .wait();
        assert!(result.is_ok());
        assert_eq!(to.lock().take().unwrap().0, 2);
    }
}
