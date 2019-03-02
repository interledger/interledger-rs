use super::SpspResponse;
use futures::future::{ok, FutureResult, IntoFuture};
use hyper::{service::Service as HttpService, Body, Error, Request, Response};
use interledger_stream::ConnectionGenerator;
use std::error::Error as StdError;
use std::fmt;

fn extract_original_url(request: &Request<Body>) -> String {
    let headers = request.headers();
    let host = headers
        .get("forwarded")
        .and_then(|header| {
            let header = header.to_str().ok()?;
            if let Some(index) = header.find(" for=") {
                let host_start = index + 5;
                (&header[host_start..]).split_whitespace().next()
            } else {
                None
            }
        })
        .or_else(|| {
            headers
                .get("x-forwarded-host")
                .and_then(|header| header.to_str().ok())
        })
        .or_else(|| headers.get("host").and_then(|header| header.to_str().ok()))
        .unwrap_or("");

    let mut url = host.to_string();
    url.push_str(request.uri().path());
    url.push_str(request.uri().query().unwrap_or(""));
    url
}

pub fn spsp_responder(ilp_address: &[u8], server_secret: &[u8]) -> SpspResponder {
    let connection_generator = ConnectionGenerator::new(ilp_address, server_secret);
    SpspResponder {
        connection_generator,
    }

    // service_fn(move |request: Request<Body>| {
    //     let original_url = extract_original_url(&request);
    //     let (destination_account, shared_secret) =
    //         generator.generate_address_and_secret(original_url.as_bytes());
    //     let response = SpspResponse {
    //         destination_account: String::from_utf8(destination_account.to_vec()).unwrap(),
    //         shared_secret: shared_secret.to_vec(),
    //     };

    //     Ok(Response::builder()
    //         .status(200)
    //         .body(Body::from(serde_json::to_string(&response).unwrap()))
    //         .unwrap())
    // })
}

#[derive(Clone)]
pub struct SpspResponder {
    connection_generator: ConnectionGenerator,
}

impl HttpService for SpspResponder {
    type ReqBody = Body;
    type ResBody = Body;
    type Error = Error;
    type Future = FutureResult<Response<Body>, Error>;

    fn call(&mut self, request: Request<Self::ReqBody>) -> Self::Future {
        let original_url = extract_original_url(&request);
        let (destination_account, shared_secret) = self
            .connection_generator
            .generate_address_and_secret(original_url.as_bytes());
        let response = SpspResponse {
            destination_account: String::from_utf8(destination_account.to_vec()).unwrap(),
            shared_secret: shared_secret.to_vec(),
        };

        ok(Response::builder()
            .status(200)
            .body(Body::from(serde_json::to_string(&response).unwrap()))
            .unwrap())
    }
}

impl IntoFuture for SpspResponder {
    type Item = Self;
    type Error = Never;
    type Future = FutureResult<Self::Item, Self::Error>;

    fn into_future(self) -> Self::Future {
        ok(self)
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
