use super::SpspResponse;
use bytes::Bytes;
use hyper::{service::Service as HttpService, Body, Error, Request, Response};
use interledger_packet::Address;
use interledger_stream::ConnectionGenerator;
use std::error::Error as StdError;
use std::{
    fmt, str,
    task::{Context, Poll},
};
use tracing::debug;

/// A Hyper::Service that responds to incoming SPSP Query requests with newly generated
/// details for a STREAM connection.
#[derive(Clone)]
pub struct SpspResponder {
    ilp_address: Address,
    connection_generator: ConnectionGenerator,
}

impl SpspResponder {
    /// Constructs a new SPSP Responder by receiving an ILP Address and a server **secret**
    pub fn new(ilp_address: Address, server_secret: Bytes) -> Self {
        let connection_generator = ConnectionGenerator::new(server_secret);
        SpspResponder {
            ilp_address,
            connection_generator,
        }
    }

    /// Returns an HTTP Response containing the destination account
    /// and shared secret for this connection
    /// These fields are generated via [Stream's `ConnectionGenerator`](../interledger_stream/struct.ConnectionGenerator.html#method.generate_address_and_secret)
    pub fn generate_http_response(&self) -> Response<Body> {
        let (destination_account, shared_secret) = self
            .connection_generator
            .generate_address_and_secret(&self.ilp_address);
        debug!(
            "Generated address and secret for: {:?}",
            destination_account
        );
        let response = SpspResponse {
            destination_account,
            shared_secret: shared_secret.to_vec(),
        };

        Response::builder()
            .header("Content-Type", "application/spsp4+json")
            .header("Cache-Control", "max-age=60")
            .status(200)
            .body(Body::from(serde_json::to_string(&response).unwrap()))
            .unwrap()
    }
}

impl HttpService<Request<Body>> for SpspResponder {
    type Response = Response<Body>;
    type Error = Error;
    type Future = futures::future::Ready<Result<Self::Response, Self::Error>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Ok(()).into()
    }

    fn call(&mut self, _request: Request<Body>) -> Self::Future {
        futures::future::ok(self.generate_http_response())
    }
}

// copied from https://github.com/hyperium/hyper/blob/master/src/common/never.rs
#[derive(Debug)]
pub enum Never {}

impl fmt::Display for Never {
    fn fmt(&self, _: &mut fmt::Formatter) -> fmt::Result {
        match *self {}
    }
}

impl StdError for Never {
    fn description(&self) -> &str {
        match *self {}
    }
}

#[cfg(test)]
mod spsp_server_test {
    use super::*;
    use std::str::FromStr;

    #[tokio::test]
    async fn spsp_response_headers() {
        let addr = Address::from_str("example.receiver").unwrap();
        let mut responder = SpspResponder::new(addr, Bytes::from(&[0; 32][..]));
        let response = responder
            .call(
                Request::builder()
                    .method("GET")
                    .uri("http://example.com")
                    .header("Accept", "application/spsp4+json")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            response.headers().get("Content-Type").unwrap(),
            "application/spsp4+json"
        );
        assert_eq!(
            response.headers().get("Cache-Control").unwrap(),
            "max-age=60"
        );
    }
}
