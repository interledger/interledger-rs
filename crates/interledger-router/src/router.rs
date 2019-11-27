use super::RouterStore;
use futures::{future::err, Future};
use interledger_packet::{ErrorCode, RejectBuilder};
use interledger_service::*;
use log::{error, trace};
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
    S: AddressStore + RouterStore,
    O: OutgoingService<S::Account> + Clone + Send + 'static,
{
    type Future = BoxedIlpFuture;

    /// Figures out the next node to pass the received Prepare packet to.
    ///
    /// Firstly, it checks if there is a direct path for that account and uses that.
    /// If not it scans through the routing table and checks if the route prefix matches
    /// the prepare packet's destination or if it's a catch-all address (i.e. empty prefix)
    fn handle_request(&mut self, request: IncomingRequest<S::Account>) -> Self::Future {
        let destination = request.prepare.destination();
        let mut next_hop = None;
        let routing_table = self.store.routing_table();
        let ilp_address = self.store.get_ilp_address();

        // Check if we have a direct path for that account or if we need to scan
        // through the routing table
        let dest: &str = &destination;
        if let Some(account_id) = routing_table.get(dest) {
            trace!(
                "Found direct route for address: \"{}\". Account: {}",
                destination,
                account_id
            );
            next_hop = Some(*account_id);
        } else if !routing_table.is_empty() {
            let mut matching_prefix = "";
            let routing_table = self.store.routing_table();
            for (ref prefix, account) in (*routing_table).iter() {
                // Check if the route prefix matches or is empty (meaning it's a catch-all address)
                if (prefix.is_empty() || dest.starts_with(prefix.as_str()))
                    && prefix.len() >= matching_prefix.len()
                {
                    next_hop.replace(account.clone());
                    matching_prefix = prefix.as_str();
                }
            }
            if let Some(account_id) = next_hop {
                trace!(
                    "Found matching route for address: \"{}\". Prefix: \"{}\", account: {}",
                    destination,
                    matching_prefix,
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
                            triggered_by: Some(&ilp_address),
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
            error!(
                "No route found for request {}: {:?}",
                {
                    // Log a warning if the global prefix does not match
                    let destination = request.prepare.destination();
                    if destination.scheme() != ilp_address.scheme()
                        && destination.scheme() != "peer"
                    {
                        format!(
                        " (warning: address does not start with the right scheme prefix, expected: \"{}\")",
                        ilp_address.scheme()
                    )
                    } else {
                        "".to_string()
                    }
                },
                request
            );
            Box::new(err(RejectBuilder {
                code: ErrorCode::F02_UNREACHABLE,
                message: &[],
                triggered_by: Some(&ilp_address),
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
    use interledger_packet::{Address, FulfillBuilder, PrepareBuilder};
    use interledger_service::outgoing_service_fn;
    use lazy_static::lazy_static;
    use parking_lot::Mutex;
    use std::collections::HashMap;
    use std::iter::FromIterator;
    use std::str::FromStr;
    use std::sync::Arc;
    use std::time::UNIX_EPOCH;
    use uuid::Uuid;

    #[derive(Debug, Clone)]
    struct TestAccount(Uuid);

    lazy_static! {
        pub static ref ALICE: Username = Username::from_str("alice").unwrap();
        pub static ref EXAMPLE_ADDRESS: Address = Address::from_str("example.alice").unwrap();
    }

    impl Account for TestAccount {
        fn id(&self) -> Uuid {
            self.0
        }

        fn username(&self) -> &Username {
            &ALICE
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

    #[derive(Clone)]
    struct TestStore {
        routes: HashMap<String, Uuid>,
    }

    impl AccountStore for TestStore {
        type Account = TestAccount;

        fn get_accounts(
            &self,
            account_ids: Vec<Uuid>,
        ) -> Box<dyn Future<Item = Vec<TestAccount>, Error = ()> + Send> {
            Box::new(ok(account_ids.into_iter().map(TestAccount).collect()))
        }

        // stub implementation (not used in these tests)
        fn get_account_id_from_username(
            &self,
            _username: &Username,
        ) -> Box<dyn Future<Item = Uuid, Error = ()> + Send> {
            Box::new(ok(Uuid::new_v4()))
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

    impl RouterStore for TestStore {
        fn routing_table(&self) -> Arc<HashMap<String, Uuid>> {
            Arc::new(self.routes.clone())
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
                from: TestAccount(Uuid::new_v4()),
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
                routes: HashMap::from_iter(
                    vec![("example.other".to_string(), Uuid::new_v4())].into_iter(),
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
                from: TestAccount(Uuid::new_v4()),
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
                    vec![("example.destination".to_string(), Uuid::new_v4())].into_iter(),
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
                from: TestAccount(Uuid::new_v4()),
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
                routes: HashMap::from_iter(vec![(String::new(), Uuid::new_v4())].into_iter()),
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
                from: TestAccount(Uuid::new_v4()),
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
                routes: HashMap::from_iter(
                    vec![("example.".to_string(), Uuid::new_v4())].into_iter(),
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
                from: TestAccount(Uuid::new_v4()),
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
        let id0 = Uuid::from_slice(&[0; 16]).unwrap();
        let id1 = Uuid::from_slice(&[1; 16]).unwrap();
        let id2 = Uuid::from_slice(&[2; 16]).unwrap();
        let to: Arc<Mutex<Option<TestAccount>>> = Arc::new(Mutex::new(None));
        let to_clone = to.clone();
        let mut router = Router::new(
            TestStore {
                routes: HashMap::from_iter(
                    vec![
                        (String::new(), id0),
                        ("example.destination".to_string(), id2),
                        ("example.".to_string(), id1),
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
                from: TestAccount(id0),
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
        assert_eq!(to.lock().take().unwrap().0, id2);
    }
}
