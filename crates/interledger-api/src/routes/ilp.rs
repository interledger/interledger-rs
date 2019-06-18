use futures::Future;
use hyper::{Body, Error, Request, Response};
use interledger_http::{HttpServerService, HttpStore};
use interledger_service::IncomingService;

pub struct IlpApi<S, T> {
    http_server_service: HttpServerService<S, T>,
}

impl_web! {
    impl<S, T> IlpApi<S, T>
    where
    S: IncomingService<T::Account> + Clone + Send + 'static,
    T: HttpStore,
    {
        pub fn new(store: T, incoming_handler: S) -> Self {
            IlpApi {
                http_server_service: HttpServerService::new(incoming_handler, store)
            }
        }

        #[post("/ilp")]
        // TODO make sure taking the body as a Vec (instead of Bytes) doesn't cause a copy
        // for some reason, it complains that Extract isn't implemented for Bytes even though tower-web says it is
        pub fn post_ilp(&self, body: Vec<u8>, authorization: String) -> impl Future<Item = Response<Body>, Error = Error> {
            // TODO don't recreate the HTTP request just to pass it in here
            let request = Request::builder()
                .header("Authorization", authorization)
                .body(Body::from(body))
                .unwrap();
            self.http_server_service.clone().handle_http_request(request)
        }
    }
}
