use super::RouterStore;
use futures::future::err;
use interledger_packet::{ErrorCode, RejectBuilder};
use interledger_service::*;

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
    S: OutgoingService<T::Account>,
    T: RouterStore,
{
    type Future = BoxedIlpFuture;

    fn handle_request(&mut self, request: IncomingRequest<T::Account>) -> Self::Future {
        let destination = request.prepare.destination();
        let mut next_hop: Option<&T::Account> = None;
        let mut max_prefix_len = 0;
        for route in self.store.routing_table() {
            // Check if the route prefix matches or is empty (meaning it's a catch-all address)
            if (route.0.is_empty() || destination.starts_with(&route.0[..]))
                && route.0.len() >= max_prefix_len
            {
                next_hop = Some(&route.1);
                max_prefix_len = route.0.len();
            }
        }

        if let Some(account) = next_hop {
            let request = request.into_outgoing(account.clone());
            Box::new(self.next.send_request(request))
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
