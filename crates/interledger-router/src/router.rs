use super::store::RouterStore;
use futures::future::err;
use interledger_packet::{ErrorCode, RejectBuilder};
use interledger_service::{AccountId, BoxedIlpFuture, Request, Service};

static PEER_PREFIX: &'static [u8] = b"peer.";

#[derive(Clone)]
pub struct Router<S, T> {
    next: S,
    store: T,
}

impl<S, T> Router<S, T>
where
    S: Service,
    T: RouterStore,
{
    pub fn new(next: S, store: T) -> Self {
        Router { next, store }
    }

    fn route_request(&self, request: Request) -> Result<Request, ()> {
        if request.to.is_some() || request.prepare.destination().starts_with(PEER_PREFIX) {
            Ok(request)
        } else {
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
                let mut request = request;
                request.to = Some(account_id);
                Ok(request)
            } else {
                Err(())
            }
        }
    }
}

impl<S, T> Service for Router<S, T>
where
    S: Service,
    T: RouterStore,
{
    type Future = BoxedIlpFuture;

    fn call(&mut self, request: Request) -> Self::Future {
        if let Ok(request) = self.route_request(request) {
            Box::new(self.next.call(request))
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
