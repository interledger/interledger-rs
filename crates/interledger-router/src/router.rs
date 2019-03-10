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
pub struct Router<S, T> {
    next: S,
    store: T,
}

impl<S, T> Router<S, T>
where
    S: OutgoingService<T::Account>,
    T: RouterStore,
{
    pub fn new(next: S, store: T) -> Self {
        Router { next, store }
    }
}

impl<S, T> IncomingService<T::Account> for Router<S, T>
where
    S: OutgoingService<T::Account> + Clone + Send + 'static,
    T: RouterStore,
{
    type Future = BoxedIlpFuture;

    fn handle_request(&mut self, request: IncomingRequest<T::Account>) -> Self::Future {
        let destination = Bytes::from(request.prepare.destination());
        let mut next_hop: Option<<T::Account as Account>::AccountId> = None;
        let routing_table = self.store.routing_table();

        // Check if we have a direct path for that account or if we need to scan through the routing table
        if let Some(account_id) = routing_table.get(&destination) {
            debug!(
                "Found direct route for address: \"{}\". Account: {}",
                str::from_utf8(&destination[..]).unwrap_or("<not utf8>"),
                account_id
            );
            next_hop = Some(*account_id);
        } else {
            let mut max_prefix_len = 0;
            for route in self.store.routing_table() {
                // Check if the route prefix matches or is empty (meaning it's a catch-all address)
                if (route.0.is_empty() || destination.starts_with(&route.0[..]))
                    && route.0.len() >= max_prefix_len
                {
                    next_hop = Some(route.1);
                    max_prefix_len = route.0.len();
                    debug!(
                        "Found matching route for address: \"{}\". Prefix: \"{}\", account: {}",
                        str::from_utf8(&destination[..]).unwrap_or("<not utf8>"),
                        str::from_utf8(&route.0[..]).unwrap_or("<not utf8>"),
                        route.1,
                    );
                }
            }
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
            debug!("No route found for request: {:?}", request);
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
