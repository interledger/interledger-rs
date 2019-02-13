use super::store::RouterStore;
use bytes::Bytes;
use futures::{
    future::{ok, result, Either},
    Future,
};
use interledger_packet::{ErrorCode, RejectBuilder};
use interledger_service::{BoxedIlpFuture, Request, Service};
use parking_lot::{Mutex, RwLock};
use std::sync::Arc;

static PEER_PREFIX: &'static [u8] = b"peer.";

#[derive(Clone)]
pub struct Router<S> {
    routes: Arc<RwLock<Vec<Route>>>,
    store: S,
}

impl<S> Router<S>
where
    S: RouterStore,
{
    pub fn new(store: S) -> Self {
        Router {
            routes: Arc::new(RwLock::new(Vec::new())),
            store,
        }
    }

    pub fn add_route<T, U>(&mut self, prefix: T, service: U) -> &mut Self
    where
        Bytes: From<T>,
        U: Service + Send + 'static,
    {
        let route = Route::new(prefix, service);
        self.routes.write().push(route);
        self
    }

    fn get_next_hop_prefix(
        &self,
        request: Request,
    ) -> impl Future<Item = (Bytes, Request), Error = ()> + 'static {
        if request.to.is_some() || request.prepare.destination().starts_with(PEER_PREFIX) {
            Either::A(ok((Bytes::from(request.prepare.destination()), request)))
        } else {
            Either::B(
                self.store
                    .get_next_hop(request.prepare.destination())
                    .and_then(|(account_id, prefix)| {
                        let mut request = request;
                        request.to = Some(account_id);
                        Ok((prefix, request))
                    }),
            )
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
                .and_then(move |(prefix, request)| {
                    result(
                        routes
                            .read()
                            .iter()
                            .find(|route| route.matches(&prefix[..]))
                            .cloned()
                            .ok_or_else(|| {
                                RejectBuilder {
                                    code: ErrorCode::F02_UNREACHABLE,
                                    message: &[],
                                    triggered_by: &[],
                                    data: &[],
                                }
                                .build()
                            }),
                    )
                    .and_then(|mut route| route.call(request))
                }),
        )
    }
}

#[derive(Clone)]
struct Route {
    prefix: Bytes,
    call_service: Arc<Box<Fn(Request) -> BoxedIlpFuture + Send + Sync>>,
}

impl Route {
    fn new<P, S>(prefix: P, service: S) -> Self
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
