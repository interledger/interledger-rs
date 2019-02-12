extern crate futures;
extern crate interledger_packet;

use futures::{Future, Poll};
use interledger_packet::{Prepare, Fulfill, Reject};

pub struct Request<'a> {
  pub from: Option<&'a [u8]>,
  pub to: Option<&'a [u8]>,
  pub prepare: Prepare,
}

pub trait Service {
  type Future: Future<Item = Fulfill, Error = Reject> + Send + 'static;

  fn poll_ready(&mut self) -> Poll<(), ()>;
  fn call(&mut self, request: Request) -> Self::Future;
}

pub type BoxedIlpFuture = Box<Future<Item = Fulfill, Error = Reject> + Send + 'static>;
