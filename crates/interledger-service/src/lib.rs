extern crate futures;
extern crate interledger_packet;

use futures::{Async, Future, Poll};
use interledger_packet::{Fulfill, Prepare, Reject};

pub type AccountId = u64;

pub struct Request {
    pub from: Option<AccountId>,
    pub to: Option<AccountId>,
    pub prepare: Prepare,
}

// TODO should services be cloneable by default? it helps with lifetime issues
pub trait Service {
    type Future: Future<Item = Fulfill, Error = Reject> + Send + 'static;

    fn poll_ready(&mut self) -> Poll<(), ()> {
        Ok(Async::Ready(()))
    }

    fn call(&mut self, request: Request) -> Self::Future;
}

pub type BoxedIlpFuture = Box<Future<Item = Fulfill, Error = Reject> + Send + 'static>;
