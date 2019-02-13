extern crate futures;
extern crate interledger_packet;

use futures::{Future, Poll};
use interledger_packet::{Fulfill, Prepare, Reject};

pub struct Request {
    pub from: Option<u64>,
    pub to: Option<u64>,
    pub prepare: Prepare,
}

// TODO should services be cloneable by default? it helps with lifetime issues
pub trait Service: Clone {
    type Future: Future<Item = Fulfill, Error = Reject> + Send + 'static;

    fn poll_ready(&mut self) -> Poll<(), ()>;
    fn call(&mut self, request: Request) -> Self::Future;
}

pub type BoxedIlpFuture = Box<Future<Item = Fulfill, Error = Reject> + Send + 'static>;
