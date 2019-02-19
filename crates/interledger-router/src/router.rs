use super::store::RouterStore;
use bytes::Bytes;
use futures::{
    future::{ok, result},
    Future,
};
use interledger_packet::{ErrorCode, RejectBuilder};
use interledger_service::{AccountId, BoxedIlpFuture, Request, Service};
use parking_lot::Mutex;
use std::sync::Arc;

static PEER_PREFIX: &'static [u8] = b"peer.";

#[derive(Clone)]
pub struct Router<S> {
    routes: Arc<Vec<Route>>,
    store: S,
}

impl<S> Router<S>
where
    S: RouterStore,
{
    pub fn new<'a, R, T>(routes: R, store: S) -> Self
    where
        R: IntoIterator<Item = (&'a [u8], Box<T>)>,
        T: Service + Send + 'static,
    {
        let routes = routes
            .into_iter()
            .map(|(prefix, service)| Route::new(prefix, service))
            .collect();
        Router {
            routes: Arc::new(routes),
            store,
        }
    }

    fn get_next_hop_prefix(&self, request: Request) -> Result<(Request, Option<&[u8]>), ()> {
        if request.to.is_some() || request.prepare.destination().starts_with(PEER_PREFIX) {
            Ok((request, None))
        } else {
            let destination = request.prepare.destination();
            let mut next_hop: Option<(AccountId, &[u8])> = None;
            let mut max_prefix_len = 0;
            for route in self.store.get_routing_table().iter() {
                if destination.starts_with(route.0) && route.0.len() > max_prefix_len {
                    next_hop = Some(route.1);
                    max_prefix_len = route.0.len();
                }
            }

            if let Some((account_id, prefix)) = next_hop {
                let mut request = request;
                request.to = Some(account_id);
                Ok((request, Some(prefix)))
            } else {
                Err(())
            }
        }
    }
}

impl<S> Service for Router<S>
where
    S: RouterStore,
{
    type Future = BoxedIlpFuture;

    fn call(&mut self, request: Request) -> Self::Future {
        let routes = self.routes.clone();
        Box::new(
            result(
                self.get_next_hop_prefix(request)
                    .map_err(|_| {
                        RejectBuilder {
                            code: ErrorCode::F02_UNREACHABLE,
                            message: &[],
                            triggered_by: &[],
                            data: &[],
                        }
                        .build()
                    })
                    .and_then(move |(request, prefix)| {
                        routes
                            .iter()
                            .find(|route| {
                                route.matches(
                                    prefix.or(Some(request.prepare.destination())).unwrap(),
                                )
                            })
                            .cloned()
                            .ok_or_else(|| {
                                RejectBuilder {
                                    code: ErrorCode::F02_UNREACHABLE,
                                    message: &[],
                                    triggered_by: &[],
                                    data: &[],
                                }
                                .build()
                            })
                            .map(|route| (route, request))
                    }),
            )
            .and_then(|(mut route, request)| route.call(request)),
        )
    }
}

#[derive(Clone)]
struct Route {
    prefix: Bytes,
    call_service: Arc<Box<Fn(Request) -> BoxedIlpFuture + Send + Sync>>,
}

impl Route {
    fn new<P, S>(prefix: P, service: Box<S>) -> Self
    where
        Bytes: From<P>,
        S: Service + Send + 'static,
    {
        let service = Arc::new(Mutex::new(service));
        Route {
            prefix: Bytes::from(prefix),
            call_service: Arc::new(Box::new(move |request: Request| {
                Box::new(service.clone().lock().call(request))
            })),
        }
    }

    fn matches(&self, address: &[u8]) -> bool {
        self.prefix.starts_with(address)
    }
}

impl Service for Route {
    type Future = BoxedIlpFuture;

    fn call(&mut self, request: Request) -> Self::Future {
        (self.call_service)(request)
    }
}
