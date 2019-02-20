use super::store::RouterStore;
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
    S: OutgoingService,
    T: RouterStore,
{
    pub fn new(next: S, store: T) -> Self {
        Router { next, store }
    }
}

impl<S, T> IncomingService for Router<S, T>
where
    S: OutgoingService,
    T: RouterStore,
{
    type Future = BoxedIlpFuture;

    fn handle_request(&mut self, request: IncomingRequest) -> Self::Future {
        let destination = request.prepare.destination();
        let mut next_hop: Option<AccountId> = None;
        let mut max_prefix_len = 0;
        for route in self.store.get_routing_table().iter() {
            if destination.starts_with(&route.0[..]) && route.0.len() > max_prefix_len {
                next_hop = Some(route.1);
                max_prefix_len = route.0.len();
            }
        }

        if let Some(account_id) = next_hop {
            let request = request.into_outgoing(account_id);
            Box::new(self.next.send_request(request))
        } else {
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
