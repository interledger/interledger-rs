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
    fmt::{self, Debug},
    marker::PhantomData,
    sync::Arc,
};
use uuid::Uuid;

mod username;
pub use username::Username;
#[cfg(feature = "trace")]
mod trace;
#[cfg(feature = "trace")]
pub use trace::*;

/// The base trait that Account types from other Services extend.
/// This trait only assumes that the account has an ID that can be compared with others.
///
/// Each service can extend the Account type to include additional details they require.
/// Store implementations will implement these Account traits for a concrete type that
/// they will load from the database.
pub trait Account: Clone + Send + Sized + Debug {
    fn id(&self) -> Uuid;
    fn username(&self) -> &Username;
    fn ilp_address(&self) -> &Address;
    fn asset_scale(&self) -> u8;
    fn asset_code(&self) -> &str;
}

/// A struct representing an incoming ILP Prepare packet or an outgoing one before the next hop is set.
#[derive(Clone)]
pub struct IncomingRequest<A: Account> {
    pub from: A,
    pub prepare: Prepare,
}

// Use a custom debug implementation to specify the order of the fields
impl<A> Debug for IncomingRequest<A>
where
    A: Account,
{
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
pub struct OutgoingRequest<A: Account> {
    pub from: A,
    pub to: A,
    pub original_amount: u64,
    pub prepare: Prepare,
}

// Use a custom debug implementation to specify the order of the fields
impl<A> Debug for OutgoingRequest<A>
where
    A: Account,
{
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

    /// Wrap the given service such that the provided function will
    /// be called to handle each request. That function can
    /// return immediately, modify the request before passing it on,
    /// and/or handle the result of calling the inner service.
    fn wrap<F, R>(self, f: F) -> WrappedService<F, Self, A>
    where
        F: Fn(IncomingRequest<A>, Self) -> R,
        R: Future<Item = Fulfill, Error = Reject> + Send + 'static,
        Self: Clone + Sized,
    {
        WrappedService::wrap_incoming(self, f)
    }
}

/// Core service trait for sending OutgoingRequests that asynchronously returns an ILP Fulfill or Reject packet.
pub trait OutgoingService<A: Account> {
    type Future: Future<Item = Fulfill, Error = Reject> + Send + 'static;

    fn send_request(&mut self, request: OutgoingRequest<A>) -> Self::Future;

    /// Wrap the given service such that the provided function will
    /// be called to handle each request. That function can
    /// return immediately, modify the request before passing it on,
    /// and/or handle the result of calling the inner service.
    fn wrap<F, R>(self, f: F) -> WrappedService<F, Self, A>
    where
        F: Fn(OutgoingRequest<A>, Self) -> R,
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
    type Account: Account;

    fn get_accounts(
        &self,
        account_ids: Vec<Uuid>,
    ) -> Box<dyn Future<Item = Vec<Self::Account>, Error = ()> + Send>;

    fn get_account_id_from_username(
        &self,
        username: &Username,
    ) -> Box<dyn Future<Item = Uuid, Error = ()> + Send>;
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

/// A service that wraps another one with a function that will be called
/// on every request.
///
/// This enables wrapping services without the boilerplate of defining a
/// new struct and implementing IncomingService and/or OutgoingService
/// every time.
#[derive(Clone)]
pub struct WrappedService<F, I, A> {
    f: F,
    inner: Arc<I>,
    account_type: PhantomData<A>,
}

impl<F, IO, A, R> WrappedService<F, IO, A>
where
    F: Fn(IncomingRequest<A>, IO) -> R,
    IO: IncomingService<A> + Clone,
    A: Account,
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
            account_type: PhantomData,
        }
    }
}

impl<F, IO, A, R> IncomingService<A> for WrappedService<F, IO, A>
where
    F: Fn(IncomingRequest<A>, IO) -> R,
    IO: IncomingService<A> + Clone,
    A: Account,
    R: Future<Item = Fulfill, Error = Reject> + Send + 'static,
{
    type Future = R;

    fn handle_request(&mut self, request: IncomingRequest<A>) -> R {
        (self.f)(request, (*self.inner).clone())
    }
}

impl<F, IO, A, R> WrappedService<F, IO, A>
where
    F: Fn(OutgoingRequest<A>, IO) -> R,
    IO: OutgoingService<A> + Clone,
    A: Account,
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
            account_type: PhantomData,
        }
    }
}

impl<F, IO, A, R> OutgoingService<A> for WrappedService<F, IO, A>
where
    F: Fn(OutgoingRequest<A>, IO) -> R,
    IO: OutgoingService<A> + Clone,
    A: Account,
    R: Future<Item = Fulfill, Error = Reject> + Send + 'static,
{
    type Future = R;

    fn send_request(&mut self, request: OutgoingRequest<A>) -> R {
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
