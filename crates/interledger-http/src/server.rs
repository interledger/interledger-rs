use bytes::{Bytes, BytesMut};
use futures::{Future, Stream};
use hyper::{body::Body, service::Service as HttpService, Error, Request, Response};
use interledger_packet::{Fulfill, Prepare, Reject};
use interledger_service::{Request as IlpRequest, Service};
use std::sync::Arc;

pub struct HttpServerService<S> {
  handler: S,
}

impl<S> HttpServerService<S>
where S: Service + 'static
{
  pub fn new(handler: S) -> Self {
    HttpServerService {
      handler,
    }
  }
}

impl<S> HttpService for HttpServerService<S>
where S: Service + 'static
{
  type ReqBody = Body;
  type ResBody = Body;
  type Error = Error;
  type Future = Box<Future<Item = Response<Self::ResBody>, Error = Self::Error> + 'static>;

  fn call(&mut self, request: Request<Self::ReqBody>) -> Self::Future {
    // Authenticate request
    let from = b"example.sender";

    // Parse packet
    Box::new(
      request
        .into_body()
        .concat2()
        .map_err(|_err| Response::builder().status(500).body(Body::empty()).unwrap())
        .and_then(|body| {
          let bytes = body.into_bytes().try_mut().unwrap_or_else(|bytes| {
            debug!("Copying bytes from incoming HTTP request into Prepare packet");
            BytesMut::from(bytes)
          });
          Prepare::new(bytes)
            .map_err(|_err| Response::builder().status(400).body(Body::empty()).unwrap())
        })
        .and_then(|prepare| {
          // Call the inner ILP service
          let request = IlpRequest {
            from: Some(from),
            to: None,
            prepare,
          };
          self.handler.call(request).then(|result| {
            // Serialize the ILP response into an HTTP response
            let bytes: BytesMut = match result {
              Ok(fulfill) => fulfill.into(),
              Err(reject) => reject.into(),
            };
            Ok(
              Response::builder()
                .status(200)
                .header("content-type", "application/octet-stream")
                .body(bytes.freeze().into())
                .unwrap(),
            )
          })
        })
        .then(|result| match result {
          Ok(response) => Ok(response),
          Err(response) => Ok(response),
        }),
    )
  }
}
