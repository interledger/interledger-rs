use futures::Future;
use interledger_packet::{Fulfill, Prepare, Reject};
use std::cmp::Eq;
use std::fmt::{Debug, Display};
use std::hash::Hash;

pub trait Account: Clone + Send + Sized + Debug {
    type AccountId: Eq + Hash + Debug + Display + Send + Sync + Copy;

    fn id(&self) -> Self::AccountId;
}

#[derive(Debug, Clone)]
pub struct IncomingRequest<A: Account> {
    pub from: A,
    pub prepare: Prepare,
}

#[derive(Debug, Clone)]
pub struct OutgoingRequest<A: Account> {
    pub from: A,
    pub to: A,
    pub prepare: Prepare,
}

impl<A> IncomingRequest<A>
where
    A: Account,
{
    pub fn into_outgoing(self, to: A) -> OutgoingRequest<A> {
        OutgoingRequest {
            from: self.from,
            prepare: self.prepare,
            to,
        }
    }
}

pub trait IncomingService<A: Account> {
    type Future: Future<Item = Fulfill, Error = Reject> + Send + 'static;

    fn handle_request(&mut self, request: IncomingRequest<A>) -> Self::Future;
}

pub trait OutgoingService<A: Account> {
    type Future: Future<Item = Fulfill, Error = Reject> + Send + 'static;

    fn send_request(&mut self, request: OutgoingRequest<A>) -> Self::Future;
}

pub type BoxedIlpFuture = Box<Future<Item = Fulfill, Error = Reject> + Send + 'static>;

pub trait AccountStore {
    type Account: Account;

    fn get_accounts(
        &self,
        account_ids: Vec<<<Self as AccountStore>::Account as Account>::AccountId>,
    ) -> Box<Future<Item = Vec<Self::Account>, Error = ()> + Send>;
}
