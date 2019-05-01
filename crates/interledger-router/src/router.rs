use super::RouterStore;
use bytes::Bytes;
use futures::{future::err, Future};
use interledger_packet::{ErrorCode, RejectBuilder};
use interledger_service::*;
use std::str;

/// The router implements the IncomingService trait and uses the routing table
/// to determine the `to` (or "next hop") Account for the given request.
///
/// Note that the router does **not**:
///   - apply exchange rates or fees to the Prepare packet
///   - adjust account balances
///   - reduce the Prepare packet's expiry
#[derive(Clone)]
pub struct Router<T, S> {
    store: T,
    next: S,
}

impl<T, S> Router<T, S>
where
    T: RouterStore,
    S: OutgoingService<T::Account>,
{
    pub fn new(store: T, next: S) -> Self {
        Router { next, store }
    }
}

impl<T, S> IncomingService<T::Account> for Router<T, S>
where
    T: RouterStore,
    S: OutgoingService<T::Account> + Clone + Send + 'static,
{
    type Future = BoxedIlpFuture;

    fn handle_request(&mut self, request: IncomingRequest<T::Account>) -> Self::Future {
        let destination = Bytes::from(request.prepare.destination());
        let mut next_hop: Option<<T::Account as Account>::AccountId> = None;
        let routing_table = self.store.routing_table();

        // Check if we have a direct path for that account or if we need to scan through the routing table
        if let Some(account_id) = routing_table.get(&destination) {
            trace!(
                "Found direct route for address: \"{}\". Account: {}",
                str::from_utf8(&destination[..]).unwrap_or("<not utf8>"),
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
                if (route.0.is_empty() || destination.starts_with(&route.0[..]))
                    && route.0.len() >= matching_prefix.len()
                {
                    next_hop.replace(route.1);
                    matching_prefix = route.0.clone();
                }
            }
            if let Some(account_id) = next_hop {
                trace!(
                    "Found matching route for address: \"{}\". Prefix: \"{}\", account: {}",
                    str::from_utf8(&destination[..]).unwrap_or("<not utf8>"),
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
                            triggered_by: &[],
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
                triggered_by: &[],
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
    use interledger_packet::{FulfillBuilder, PrepareBuilder};
    use interledger_service::outgoing_service_fn;
    use parking_lot::Mutex;
    use std::iter::FromIterator;
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
        ) -> Box<Future<Item = Vec<TestAccount>, Error = ()> + Send> {
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
                    destination: b"example.destination",
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
                    destination: b"example.destination",
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
                    destination: b"example.destination",
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
                    destination: b"example.destination",
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
                    destination: b"example.destination",
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
                    destination: b"example.destination",
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
