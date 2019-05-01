use super::limit_stream::LimitStream;
use super::HttpStore;
use bytes::BytesMut;
use futures::{
    future::{err, Either},
    Future, Stream,
};
use hyper::{
    body::Body, header::AUTHORIZATION, service::Service as HttpService, Error, Request, Response,
};
use interledger_packet::{Fulfill, Prepare, Reject};
use interledger_service::*;

/// Max message size that is allowed to transfer from a request or a message.
pub const MAX_MESSAGE_SIZE: usize = 40000;
const BEARER_TOKEN_START: usize = 7;

/// A Hyper::Service that parses incoming ILP-Over-HTTP requests, validates the authorization,
/// and passes the request to an IncomingService handler.
#[derive(Clone)]
pub struct HttpServerService<S, T> {
    next: S,
    store: T,
}

impl<S, T> HttpServerService<S, T>
where
    S: IncomingService<T::Account> + Clone + 'static,
    T: HttpStore,
{
    pub fn new(next: S, store: T) -> Self {
        HttpServerService { next, store }
    }

    // TODO support certificate-based authentication
    fn check_authorization(
        &self,
        request: &Request<Body>,
    ) -> impl Future<Item = T::Account, Error = Response<Body>> {
        let authorization: Option<String> = request
            .headers()
            .get(AUTHORIZATION)
            .and_then(|auth| auth.to_str().ok())
            .map(|auth| auth[BEARER_TOKEN_START..].to_string());
        if let Some(authorization) = authorization {
            Either::A(
                self.store
                    .get_account_from_http_token(&authorization)
                    .map_err(move |_err| {
                        error!("Authorization not found in the DB: {}", authorization);
                        Response::builder().status(401).body(Body::empty()).unwrap()
                    }),
            )
        } else {
            Either::B(err(Response::builder()
                .status(401)
                .body(Body::empty())
                .unwrap()))
        }
    }

    pub fn handle_http_request(
        &mut self,
        request: Request<Body>,
    ) -> impl Future<Item = Response<Body>, Error = Error> {
        let mut next = self.next.clone();
        self.check_authorization(&request)
            .and_then(|from_account| {
                parse_prepare_from_request(request, Some(MAX_MESSAGE_SIZE)).and_then(
                    move |prepare| {
                    trace!(
                        "Got incoming ILP over HTTP packet from account: {}",
                        from_account.id()
                    );
                        // Call the inner ILP service
                        next.handle_request(IncomingRequest {
                            from: from_account,
                            prepare,
                        })
                        .then(ilp_response_to_http_response)
                    },
                )
            })
            .then(|result| match result {
                Ok(response) => Ok(response),
                Err(response) => Ok(response),
            })
    }
}

impl<S, T> HttpService for HttpServerService<S, T>
where
    S: IncomingService<T::Account> + Clone + Send + 'static,
    T: HttpStore + 'static,
{
    type ReqBody = Body;
    type ResBody = Body;
    type Error = Error;
    type Future = Box<Future<Item = Response<Self::ResBody>, Error = Self::Error> + Send + 'static>;

    fn call(&mut self, request: Request<Self::ReqBody>) -> Self::Future {
        Box::new(self.handle_http_request(request))
    }
}

fn parse_prepare_from_request(
    request: Request<Body>,
    max_message_size: Option<usize>,
) -> impl Future<Item = Prepare, Error = Response<Body>> + 'static {
    LimitStream::new(max_message_size, request.into_body())
        .concat2()
        .map_err(|err| {
            eprintln!("Concatenating stream failed: {:?}", err);
            Response::builder().status(500).body(Body::empty()).unwrap()
        })
        .and_then(|body| {
            let bytes = body.into_bytes().try_mut().unwrap_or_else(|bytes| {
                debug!("Copying bytes from incoming HTTP request into Prepare packet");
                BytesMut::from(bytes)
            });
            Prepare::try_from(bytes).map_err(|err| {
                eprintln!("Parsing prepare packet failed: {:?}", err);
                Response::builder().status(400).body(Body::empty()).unwrap()
            })
        })
}

fn ilp_response_to_http_response(
    result: Result<Fulfill, Reject>,
) -> Result<Response<Body>, Response<Body>> {
    let bytes: BytesMut = match result {
        Ok(fulfill) => fulfill.into(),
        Err(reject) => reject.into(),
    };
    Ok(Response::builder()
        .status(200)
        .header("content-type", "application/octet-stream")
        .body(bytes.freeze().into())
        .unwrap())
}

#[cfg(test)]
mod test_limit_stream {
    use super::*;
    use interledger_packet::PrepareBuilder;
    use std::time::{Duration, SystemTime};

    #[test]
    fn test_parse_prepare_from_request_less() {
        // just ensuring that body size is more than default limit of MAX_MESSAGE_SIZE
        let prepare_data = PrepareBuilder {
            amount: 1,
            destination: b"test.prepare",
            execution_condition: &[0; 32],
            expires_at: SystemTime::now() + Duration::from_secs(30),
            data: &[0; MAX_MESSAGE_SIZE],
        };
        let prepare = prepare_data.clone().build();
        let body_size = BytesMut::from(prepare.clone()).len();
        let parsed_prepare = make_prepare_and_parse(prepare_data, Some(body_size)).unwrap();

        assert_eq!(prepare.amount(), parsed_prepare.amount());
        assert_eq!(prepare.destination(), parsed_prepare.destination());
        assert_eq!(
            prepare.execution_condition(),
            parsed_prepare.execution_condition()
        );
        assert_eq!(
            get_millis_from_unix_epoch(prepare.expires_at()),
            get_millis_from_unix_epoch(parsed_prepare.expires_at())
        );
        assert_eq!(prepare.data(), parsed_prepare.data());
    }

    #[test]
    fn test_parse_prepare_from_request_more() {
        let prepare_data = PrepareBuilder {
            amount: 1,
            destination: b"test.prepare",
            execution_condition: &[0; 32],
            expires_at: SystemTime::now() + Duration::from_secs(30),
            data: &[0; 0],
        };
        let prepare = make_prepare_and_parse(prepare_data, Some(1));
        assert!(prepare.is_err());
    }

    #[test]
    fn test_parse_prepare_from_request_no_limit() {
        let prepare_data = PrepareBuilder {
            amount: 1,
            destination: b"test.prepare",
            execution_condition: &[0; 32],
            expires_at: SystemTime::now() + Duration::from_secs(30),
            data: &[0; MAX_MESSAGE_SIZE],
        };
        let prepare = prepare_data.clone().build();
        let parsed_prepare = make_prepare_and_parse(prepare_data, None).unwrap();

        assert_eq!(prepare.amount(), parsed_prepare.amount());
        assert_eq!(prepare.destination(), parsed_prepare.destination());
        assert_eq!(
            prepare.execution_condition(),
            parsed_prepare.execution_condition()
        );
        assert_eq!(
            get_millis_from_unix_epoch(prepare.expires_at()),
            get_millis_from_unix_epoch(parsed_prepare.expires_at())
        );
        assert_eq!(prepare.data(), parsed_prepare.data());
    }

    fn make_prepare_and_parse(
        prepare_data: PrepareBuilder,
        max_message_size: Option<usize>,
    ) -> Result<Prepare, Response<Body>> {
        let prepare = prepare_data.build();
        let prepare_bytes = BytesMut::from(prepare).freeze();
        println!("prepare_bytes: {:?}", prepare_bytes);

        let body: Body = hyper::Body::from(prepare_bytes);
        let request = hyper::Request::builder()
            .header("content-type", "application/octet-stream")
            .body(body)
            .unwrap();

        parse_prepare_from_request(request, max_message_size).wait()
    }

    fn get_millis_from_unix_epoch(system_time: SystemTime) -> u128 {
        system_time
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_millis()
    }
}
