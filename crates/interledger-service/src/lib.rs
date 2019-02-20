extern crate futures;
extern crate interledger_packet;

use futures::Future;
use interledger_packet::{Fulfill, Prepare, Reject};

pub type AccountId = u64;

pub struct IncomingRequest {
    pub from: AccountId,
    pub prepare: Prepare,
}

pub struct OutgoingRequest {
    pub from: AccountId,
    pub to: AccountId,
    pub prepare: Prepare,
}

impl IncomingRequest {
    pub fn into_outgoing(self, to: AccountId) -> OutgoingRequest {
        OutgoingRequest {
            from: self.from,
            prepare: self.prepare,
            to,
        }
    }
}

pub trait IncomingService {
    type Future: Future<Item = Fulfill, Error = Reject> + Send + 'static;

    fn handle_request(&mut self, request: IncomingRequest) -> Self::Future;
}

pub trait OutgoingService {
    type Future: Future<Item = Fulfill, Error = Reject> + Send + 'static;

    fn send_request(&mut self, request: OutgoingRequest) -> Self::Future;
}

pub type BoxedIlpFuture = Box<Future<Item = Fulfill, Error = Reject> + Send + 'static>;
