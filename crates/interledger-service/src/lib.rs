//! # interledger-service
//!
//! This is the core abstraction used across the Interledger.rs implementation.
//!
//! Inspired by [tower](https://github.com/tower-rs), all of the components of this implementation are "services"
//! that take a request type and asynchronously return a result. Every component uses the same interface so that
//! services can be reused and combined into different bundles of functionality.
//!
//! The Interledger service traits use requests that contain ILP Prepare packets and the related `from`/`to` Accounts
//! and asynchronously return either an ILP Fullfill or Reject packet. Implementations of Stores (wrappers around
//! databases) can attach additional information to the Account records, which are then passed through the service chain.
//!
//! ## Example Service Bundles
//!
//! The following examples illustrate how different Services can be chained together to create different bundles of functionality.
//!
//! ### SPSP Sender
//!
//! SPSP Client --> ValidatorService --> RouterService --> HttpOutgoingService
//!
//! ### Connector
//!
//! HttpServerService --> ValidatorService --> RouterService --> BalanceAndExchangeRateService --> ValidatorService --> HttpOutgoingService
//!
//! ### STREAM Receiver
//!
//! HttpServerService --> ValidatorService --> StreamReceiverService

use futures::{Future, IntoFuture};
use interledger_packet::{Fulfill, Prepare, Reject};
use std::{
    cmp::Eq,
    fmt::{Debug, Display},
    hash::Hash,
    marker::PhantomData,
    str::FromStr,
};

/// The base trait that Account types from other Services extend.
/// This trait only assumes that the account has an ID that can be compared with others.
///
/// Each service can extend the Account type to include additional details they require.
/// Store implementations will implement these Account traits for a concrete type that
/// they will load from the database.
pub trait Account: Clone + Send + Sized + Debug {
    type AccountId: Eq + Hash + Debug + Display + Default + FromStr + Send + Sync + Copy;

    fn id(&self) -> Self::AccountId;
}

/// A struct representing an incoming ILP Prepare packet or an outgoing one before the next hop is set.
#[derive(Debug, Clone)]
pub struct IncomingRequest<A: Account> {
    pub from: A,
    pub prepare: Prepare,
}

/// A struct representing an ILP Prepare packet with the incoming and outgoing accounts set.
#[derive(Debug, Clone)]
pub struct OutgoingRequest<A: Account> {
    pub from: A,
    pub to: A,
    pub original_amount: u64,
    pub prepare: Prepare,
}

/// Set the `to` Account and turn this into an OutgoingRequest
impl<A> IncomingRequest<A>
where
    A: Account,
{
    pub fn into_outgoing(self, to: A) -> OutgoingRequest<A> {
        OutgoingRequest {
            from: self.from,
            original_amount: self.prepare.amount(),
            prepare: self.prepare,
            to,
        }
    }
}

/// Core service trait for handling IncomingRequests that asynchronously returns an ILP Fulfill or Reject packet.
pub trait IncomingService<A: Account> {
    type Future: Future<Item = Fulfill, Error = Reject> + Send + 'static;

    fn handle_request(&mut self, request: IncomingRequest<A>) -> Self::Future;
}

/// Core service trait for sending OutgoingRequests that asynchronously returns an ILP Fulfill or Reject packet.
pub trait OutgoingService<A: Account> {
    type Future: Future<Item = Fulfill, Error = Reject> + Send + 'static;

    fn send_request(&mut self, request: OutgoingRequest<A>) -> Self::Future;
}

/// A future that returns an ILP Fulfill or Reject packet.
pub type BoxedIlpFuture = Box<Future<Item = Fulfill, Error = Reject> + Send + 'static>;

/// The base Store trait that can load a given account based on the ID.
pub trait AccountStore {
    type Account: Account;

    fn get_accounts(
        &self,
        account_ids: Vec<<<Self as AccountStore>::Account as Account>::AccountId>,
    ) -> Box<Future<Item = Vec<Self::Account>, Error = ()> + Send>;
}

/// Create an IncomingService that calls the given handler for each request.
pub fn incoming_service_fn<A, B, F>(handler: F) -> ServiceFn<F, A>
where
    A: Account,
    B: IntoFuture<Item = Fulfill, Error = Reject>,
    F: FnMut(IncomingRequest<A>) -> B,
{
    ServiceFn {
        handler,
        account_type: PhantomData,
    }
}

/// Create an OutgoingService that calls the given handler for each request.
pub fn outgoing_service_fn<A, B, F>(handler: F) -> ServiceFn<F, A>
where
    A: Account,
    B: IntoFuture<Item = Fulfill, Error = Reject>,
    F: FnMut(OutgoingRequest<A>) -> B,
{
    ServiceFn {
        handler,
        account_type: PhantomData,
    }
}

/// A service created by `incoming_service_fn` or `outgoing_service_fn`
#[derive(Clone)]
pub struct ServiceFn<F, A> {
    handler: F,
    account_type: PhantomData<A>,
}

impl<F, A, B> IncomingService<A> for ServiceFn<F, A>
where
    A: Account,
    B: IntoFuture<Item = Fulfill, Error = Reject>,
    <B as futures::future::IntoFuture>::Future: std::marker::Send + 'static,
    F: FnMut(IncomingRequest<A>) -> B,
{
    type Future = BoxedIlpFuture;

    fn handle_request(&mut self, request: IncomingRequest<A>) -> Self::Future {
        Box::new((self.handler)(request).into_future())
    }
}

impl<F, A, B> OutgoingService<A> for ServiceFn<F, A>
where
    A: Account,
    B: IntoFuture<Item = Fulfill, Error = Reject>,
    <B as futures::future::IntoFuture>::Future: std::marker::Send + 'static,
    F: FnMut(OutgoingRequest<A>) -> B,
{
    type Future = BoxedIlpFuture;

    fn send_request(&mut self, request: OutgoingRequest<A>) -> Self::Future {
        Box::new((self.handler)(request).into_future())
    }
}
