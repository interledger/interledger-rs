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
use interledger_packet::{Address, Fulfill, Prepare, Reject};
use std::{
    cmp::Eq,
    fmt::{self, Debug, Display},
    hash::Hash,
    marker::PhantomData,
    str::FromStr,
    sync::Arc,
};

use serde::{Deserialize, Serialize};

mod auth;
pub use auth::{Auth as AuthToken, Username};
#[cfg(feature = "trace")]
mod trace;
#[cfg(feature = "trace")]
pub use trace::*;

mod account;
pub use account::*;

mod crypto;
pub use crypto::*;

/// Data structure used to describe the routing relation of an account with its peers.
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, PartialOrd, Serialize, Deserialize)]
pub enum RoutingRelation {
    /// An account from which we do not receive routes from, neither broadcast
    /// routes to
    NonRoutingAccount = 0,
    /// An account from which we receive routes from, but do not broadcast
    /// routes to
    Parent = 1,
    /// An account from which we receive routes from and broadcast routes to
    Peer = 2,
    /// An account from which we do not receive routes from, but broadcast
    /// routes to
    Child = 3,
}

impl FromStr for RoutingRelation {
    type Err = ();

    fn from_str(string: &str) -> Result<Self, ()> {
        match string.to_lowercase().as_str() {
            "nonroutingaccount" => Ok(RoutingRelation::NonRoutingAccount),
            "parent" => Ok(RoutingRelation::Parent),
            "peer" => Ok(RoutingRelation::Peer),
            "child" => Ok(RoutingRelation::Child),
            _ => Err(()),
        }
    }
}

impl AsRef<str> for RoutingRelation {
    fn as_ref(&self) -> &'static str {
        match self {
            RoutingRelation::NonRoutingAccount => "NonRoutingAccount",
            RoutingRelation::Parent => "Parent",
            RoutingRelation::Peer => "Peer",
            RoutingRelation::Child => "Child",
        }
    }
}

impl fmt::Display for RoutingRelation {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}", self.as_ref())
    }
}

/// A struct representing an incoming ILP Prepare packet or an outgoing one before the next hop is set.
#[derive(Clone)]
pub struct IncomingRequest {
    pub from: Account,
    pub prepare: Prepare,
}

// Use a custom debug implementation to specify the order of the fields
impl Debug for IncomingRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        formatter
            .debug_struct("IncomingRequest")
            .field("prepare", &self.prepare)
            .field("from", &self.from)
            .finish()
    }
}

/// A struct representing an ILP Prepare packet with the incoming and outgoing accounts set.
#[derive(Clone)]
pub struct OutgoingRequest {
    pub from: Account,
    pub to: Account,
    pub original_amount: u64,
    pub prepare: Prepare,
}

// Use a custom debug implementation to specify the order of the fields
impl Debug for OutgoingRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        formatter
            .debug_struct("OutgoingRequest")
            .field("prepare", &self.prepare)
            .field("original_amount", &self.original_amount)
            .field("to", &self.to)
            .field("from", &self.from)
            .finish()
    }
}

/// Set the `to` Account and turn this into an OutgoingRequest
impl IncomingRequest {
    pub fn into_outgoing(self, to: Account) -> OutgoingRequest {
        OutgoingRequest {
            from: self.from,
            original_amount: self.prepare.amount(),
            prepare: self.prepare,
            to,
        }
    }
}

/// Core service trait for handling IncomingRequests that asynchronously returns an ILP Fulfill or Reject packet.
pub trait IncomingService {
    type Future: Future<Item = Fulfill, Error = Reject> + Send + 'static;

    fn handle_request(&mut self, request: IncomingRequest) -> Self::Future;

    /// Wrap the given service such that the provided function will
    /// be called to handle each request. That function can
    /// return immediately, modify the request before passing it on,
    /// and/or handle the result of calling the inner service.
    fn wrap<F, R>(self, f: F) -> WrappedService<F, Self>
    where
        F: Fn(IncomingRequest, Self) -> R,
        R: Future<Item = Fulfill, Error = Reject> + Send + 'static,
        Self: Clone + Sized,
    {
        WrappedService::wrap_incoming(self, f)
    }
}

/// Core service trait for sending OutgoingRequests that asynchronously returns an ILP Fulfill or Reject packet.
pub trait OutgoingService {
    type Future: Future<Item = Fulfill, Error = Reject> + Send + 'static;

    fn send_request(&mut self, request: OutgoingRequest) -> Self::Future;

    /// Wrap the given service such that the provided function will
    /// be called to handle each request. That function can
    /// return immediately, modify the request before passing it on,
    /// and/or handle the result of calling the inner service.
    fn wrap<F, R>(self, f: F) -> WrappedService<F, Self>
    where
        F: Fn(OutgoingRequest, Self) -> R,
        R: Future<Item = Fulfill, Error = Reject> + Send + 'static,
        Self: Clone + Sized,
    {
        WrappedService::wrap_outgoing(self, f)
    }
}

/// A future that returns an ILP Fulfill or Reject packet.
pub type BoxedIlpFuture = Box<dyn Future<Item = Fulfill, Error = Reject> + Send + 'static>;

/// The base Store trait that can load a given account based on the ID.
pub trait AccountStore {
    fn get_accounts(
        &self,
        account_ids: Vec<AccountId>,
    ) -> Box<dyn Future<Item = Vec<Account>, Error = ()> + Send>;

    fn get_account_id_from_username(
        &self,
        username: &Username,
    ) -> Box<dyn Future<Item = AccountId, Error = ()> + Send>;
}

/// Create an IncomingService that calls the given handler for each request.
pub fn incoming_service_fn<B, F>(handler: F) -> ServiceFn<F>
where
    B: IntoFuture<Item = Fulfill, Error = Reject>,
    F: FnMut(IncomingRequest) -> B,
{
    ServiceFn { handler }
}

/// Create an OutgoingService that calls the given handler for each request.
pub fn outgoing_service_fn<B, F>(handler: F) -> ServiceFn<F>
where
    B: IntoFuture<Item = Fulfill, Error = Reject>,
    F: FnMut(OutgoingRequest) -> B,
{
    ServiceFn { handler }
}

/// A service created by `incoming_service_fn` or `outgoing_service_fn`
#[derive(Clone)]
pub struct ServiceFn<F> {
    handler: F,
}

impl<F, B> IncomingService for ServiceFn<F>
where
    B: IntoFuture<Item = Fulfill, Error = Reject>,
    <B as futures::future::IntoFuture>::Future: std::marker::Send + 'static,
    F: FnMut(IncomingRequest) -> B,
{
    type Future = BoxedIlpFuture;

    fn handle_request(&mut self, request: IncomingRequest) -> Self::Future {
        Box::new((self.handler)(request).into_future())
    }
}

impl<F, B> OutgoingService for ServiceFn<F>
where
    B: IntoFuture<Item = Fulfill, Error = Reject>,
    <B as futures::future::IntoFuture>::Future: std::marker::Send + 'static,
    F: FnMut(OutgoingRequest) -> B,
{
    type Future = BoxedIlpFuture;

    fn send_request(&mut self, request: OutgoingRequest) -> Self::Future {
        Box::new((self.handler)(request).into_future())
    }
}

/// A service that wraps another one with a function that will be called
/// on every request.
///
/// This enables wrapping services without the boilerplate of defining a
/// new struct and implementing IncomingService and/or OutgoingService
/// every time.
#[derive(Clone)]
pub struct WrappedService<F, I> {
    f: F,
    inner: Arc<I>,
}

impl<F, IO, R> WrappedService<F, IO>
where
    F: Fn(IncomingRequest, IO) -> R,
    IO: IncomingService + Clone,
    R: Future<Item = Fulfill, Error = Reject> + Send + 'static,
{
    /// Wrap the given service such that the provided function will
    /// be called to handle each request. That function can
    /// return immediately, modify the request before passing it on,
    /// and/or handle the result of calling the inner service.
    pub fn wrap_incoming(inner: IO, f: F) -> Self {
        WrappedService {
            f,
            inner: Arc::new(inner),
        }
    }
}

impl<F, IO, R> IncomingService for WrappedService<F, IO>
where
    F: Fn(IncomingRequest, IO) -> R,
    IO: IncomingService + Clone,
    R: Future<Item = Fulfill, Error = Reject> + Send + 'static,
{
    type Future = R;

    fn handle_request(&mut self, request: IncomingRequest) -> R {
        (self.f)(request, (*self.inner).clone())
    }
}

impl<F, IO, R> WrappedService<F, IO>
where
    F: Fn(OutgoingRequest, IO) -> R,
    IO: OutgoingService + Clone,
    R: Future<Item = Fulfill, Error = Reject> + Send + 'static,
{
    /// Wrap the given service such that the provided function will
    /// be called to handle each request. That function can
    /// return immediately, modify the request before passing it on,
    /// and/or handle the result of calling the inner service.
    pub fn wrap_outgoing(inner: IO, f: F) -> Self {
        WrappedService {
            f,
            inner: Arc::new(inner),
        }
    }
}

impl<F, IO, R> OutgoingService for WrappedService<F, IO>
where
    F: Fn(OutgoingRequest, IO) -> R,
    IO: OutgoingService + Clone,
    R: Future<Item = Fulfill, Error = Reject> + Send + 'static,
{
    type Future = R;

    fn send_request(&mut self, request: OutgoingRequest) -> R {
        (self.f)(request, (*self.inner).clone())
    }
}

pub trait AddressStore: Clone {
    /// Saves the ILP Address in the store's memory and database
    fn set_ilp_address(
        &self,
        ilp_address: Address,
    ) -> Box<dyn Future<Item = (), Error = ()> + Send>;

    fn clear_ilp_address(&self) -> Box<dyn Future<Item = (), Error = ()> + Send>;

    /// Get's the store's ilp address from memory
    fn get_ilp_address(&self) -> Address;
}
