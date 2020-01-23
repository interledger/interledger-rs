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

use async_trait::async_trait;
use interledger_packet::{Address, Fulfill, Prepare, Reject};
use std::{
    fmt::{self, Debug},
    future::Future,
    marker::PhantomData,
    sync::Arc,
};
use uuid::Uuid;

mod username;
pub use username::Username;
#[cfg(feature = "trace")]
mod trace;

/// Result wrapper over [Fulfill](../interledger_packet/struct.Fulfill.html) and [Reject](../interledger_packet/struct.Reject.html)
pub type IlpResult = Result<Fulfill, Reject>;

/// The base trait that Account types from other Services extend.
/// This trait assumes that the account has an ID that can be compared with others.
/// An account is also characterized by its username, ILP Address, and asset details (the code and the scale)
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
    /// The account which the request originates from
    pub from: A,
    /// The prepare packet attached to the request
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
    /// The account which the request originates from
    pub from: A,
    /// The account which the packet is being sent to
    pub to: A,
    /// The amount attached to the packet by its original sender
    pub original_amount: u64,
    /// The prepare packet attached to the request
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
#[async_trait]
pub trait IncomingService<A: Account> {
    /// Receives an Incoming request, and modifies it in place and passes it
    /// to the next service. Alternatively, if the packet was intended for the service,
    /// it returns an ILP Fulfill or Reject packet.
    async fn handle_request(&mut self, request: IncomingRequest<A>) -> IlpResult;

    /// Wrap the given service such that the provided function will
    /// be called to handle each request. That function can
    /// return immediately, modify the request before passing it on,
    /// and/or handle the result of calling the inner service.
    fn wrap<F, R>(self, f: F) -> WrappedService<F, Self, A>
    where
        F: Fn(IncomingRequest<A>, Self) -> R,
        R: Future<Output = IlpResult> + Send + 'static,
        Self: Clone + Sized,
    {
        WrappedService::wrap_incoming(self, f)
    }
}

/// Core service trait for sending OutgoingRequests that asynchronously returns an ILP Fulfill or Reject packet.
#[async_trait]
pub trait OutgoingService<A: Account> {
    /// Receives an Outgoing request, and modifies it in place and passes it
    /// to the next service. Alternatively, if the packet was intended for the service,
    /// it returns an ILP Fulfill or Reject packet.
    async fn send_request(&mut self, request: OutgoingRequest<A>) -> IlpResult;

    /// Wrap the given service such that the provided function will
    /// be called to handle each request. That function can
    /// return immediately, modify the request before passing it on,
    /// and/or handle the result of calling the inner service.
    fn wrap<F, R>(self, f: F) -> WrappedService<F, Self, A>
    where
        F: Fn(OutgoingRequest<A>, Self) -> R,
        R: Future<Output = IlpResult> + Send + 'static,
        Self: Clone + Sized,
    {
        WrappedService::wrap_outgoing(self, f)
    }
}

/// The base Store trait that can load a given account based on the ID.
#[async_trait]
pub trait AccountStore {
    /// The provided account type. Must implement the `Account` trait.
    type Account: Account;

    /// Loads the accounts which correspond to the provided account ids
    async fn get_accounts(
        &self,
        // The account ids (UUID format) of the accounts you are fetching
        account_ids: Vec<Uuid>,
    ) -> Result<Vec<Self::Account>, ()>;

    /// Loads the account id which corresponds to the provided username
    async fn get_account_id_from_username(
        &self,
        // The username of the account you are fetching
        username: &Username,
    ) -> Result<Uuid, ()>;
}

/// Create an IncomingService that calls the given handler for each request.
pub fn incoming_service_fn<A, F>(handler: F) -> ServiceFn<F, A>
where
    A: Account,
    F: FnMut(IncomingRequest<A>) -> IlpResult,
{
    ServiceFn {
        handler,
        account_type: PhantomData,
    }
}

/// Create an OutgoingService that calls the given handler for each request.
pub fn outgoing_service_fn<A, F>(handler: F) -> ServiceFn<F, A>
where
    A: Account,
    F: FnMut(OutgoingRequest<A>) -> IlpResult,
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

#[async_trait]
impl<F, A> IncomingService<A> for ServiceFn<F, A>
where
    A: Account,
    F: FnMut(IncomingRequest<A>) -> IlpResult + Send,
{
    async fn handle_request(&mut self, request: IncomingRequest<A>) -> IlpResult {
        (self.handler)(request)
    }
}

#[async_trait]
impl<F, A> OutgoingService<A> for ServiceFn<F, A>
where
    A: Account,
    F: FnMut(OutgoingRequest<A>) -> IlpResult + Send,
{
    async fn send_request(&mut self, request: OutgoingRequest<A>) -> IlpResult {
        (self.handler)(request)
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
    R: Future<Output = IlpResult> + Send + 'static,
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

#[async_trait]
impl<F, IO, A, R> IncomingService<A> for WrappedService<F, IO, A>
where
    F: Fn(IncomingRequest<A>, IO) -> R + Send + Sync,
    IO: IncomingService<A> + Send + Sync + Clone,
    A: Account,
    R: Future<Output = IlpResult> + Send + 'static,
{
    async fn handle_request(&mut self, request: IncomingRequest<A>) -> IlpResult {
        (self.f)(request, (*self.inner).clone()).await
    }
}

impl<F, IO, A, R> WrappedService<F, IO, A>
where
    F: Fn(OutgoingRequest<A>, IO) -> R,
    IO: OutgoingService<A> + Clone,
    A: Account,
    R: Future<Output = IlpResult> + Send + 'static,
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

#[async_trait]
impl<F, IO, A, R> OutgoingService<A> for WrappedService<F, IO, A>
where
    F: Fn(OutgoingRequest<A>, IO) -> R + Send + Sync,
    IO: OutgoingService<A> + Clone + Send + Sync,
    A: Account,
    R: Future<Output = IlpResult> + Send + 'static,
{
    async fn send_request(&mut self, request: OutgoingRequest<A>) -> IlpResult {
        (self.f)(request, (*self.inner).clone()).await
    }
}

/// A store responsible for managing the node's ILP Address. When
/// an account is added as a parent via the REST API, the node will
/// perform an ILDCP request to it. The parent will then return the ILP Address
/// which has been assigned to the node. The node will then proceed to set its
/// ILP Address to that value.
#[async_trait]
pub trait AddressStore: Clone {
    /// Saves the ILP Address in the database AND in the store's memory so that it can
    /// be read without read overhead
    async fn set_ilp_address(
        &self,
        // The new ILP Address of the node
        ilp_address: Address,
    ) -> Result<(), ()>;

    /// Resets the node's ILP Address to local.host
    async fn clear_ilp_address(&self) -> Result<(), ()>;

    /// Gets the node's ILP Address *synchronously*
    /// (the value is stored in memory because it is read often by all services)
    fn get_ilp_address(&self) -> Address;
}
