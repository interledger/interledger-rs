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
use interledger_errors::{AccountStoreError, AddressStoreError};
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
        F: Send + Sync + Fn(IncomingRequest<A>, Box<dyn IncomingService<A> + Send>) -> R,
        R: Future<Output = IlpResult>,
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
        F: Send + Sync + Fn(OutgoingRequest<A>, Box<dyn OutgoingService<A> + Send>) -> R,
        R: Future<Output = IlpResult>,
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
    ) -> Result<Vec<Self::Account>, AccountStoreError>;

    /// Loads the account id which corresponds to the provided username
    async fn get_account_id_from_username(
        &self,
        // The username of the account you are fetching
        username: &Username,
    ) -> Result<Uuid, AccountStoreError>;
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

impl<F, I, A, R> WrappedService<F, I, A>
where
    F: Send + Sync + Fn(IncomingRequest<A>, Box<dyn IncomingService<A> + Send>) -> R,
    R: Future<Output = IlpResult>,
    I: IncomingService<A> + Clone,
    A: Account,
{
    /// Wrap the given service such that the provided function will
    /// be called to handle each request. That function can
    /// return immediately, modify the request before passing it on,
    /// and/or handle the result of calling the inner service.
    pub fn wrap_incoming(inner: I, f: F) -> Self {
        WrappedService {
            f,
            inner: Arc::new(inner),
            account_type: PhantomData,
        }
    }
}

#[async_trait]
impl<F, I, A, R> IncomingService<A> for WrappedService<F, I, A>
where
    F: Send + Sync + Fn(IncomingRequest<A>, Box<dyn IncomingService<A> + Send>) -> R,
    R: Future<Output = IlpResult> + Send + 'static,
    I: IncomingService<A> + Send + Sync + Clone + 'static,
    A: Account + Sync,
{
    async fn handle_request(&mut self, request: IncomingRequest<A>) -> IlpResult {
        (self.f)(request, Box::new((*self.inner).clone())).await
    }
}

impl<F, O, A, R> WrappedService<F, O, A>
where
    F: Send + Sync + Fn(OutgoingRequest<A>, Box<dyn OutgoingService<A> + Send>) -> R,
    R: Future<Output = IlpResult>,
    O: OutgoingService<A> + Clone,
    A: Account,
{
    /// Wrap the given service such that the provided function will
    /// be called to handle each request. That function can
    /// return immediately, modify the request before passing it on,
    /// and/or handle the result of calling the inner service.
    pub fn wrap_outgoing(inner: O, f: F) -> Self {
        WrappedService {
            f,
            inner: Arc::new(inner),
            account_type: PhantomData,
        }
    }
}

#[async_trait]
impl<F, O, A, R> OutgoingService<A> for WrappedService<F, O, A>
where
    F: Send + Sync + Fn(OutgoingRequest<A>, Box<dyn OutgoingService<A> + Send>) -> R,
    R: Future<Output = IlpResult> + Send + 'static,
    O: OutgoingService<A> + Send + Sync + Clone + 'static,
    A: Account,
{
    async fn send_request(&mut self, request: OutgoingRequest<A>) -> IlpResult {
        (self.f)(request, Box::new((*self.inner).clone())).await
    }
}

/// A store responsible for managing the node's ILP Address. When
/// an account is added as a parent via the REST API, the node will
/// perform an ILDCP request to it. The parent will then return the ILP Address
/// which has been assigned to the node. The node will then proceed to set its
/// ILP Address to that value.
#[async_trait]
pub trait AddressStore {
    /// Saves the ILP Address in the database AND in the store's memory so that it can
    /// be read without read overhead
    async fn set_ilp_address(
        &self,
        // The new ILP Address of the node
        ilp_address: Address,
    ) -> Result<(), AddressStoreError>;

    /// Resets the node's ILP Address to local.host
    async fn clear_ilp_address(&self) -> Result<(), AddressStoreError>;

    /// Gets the node's ILP Address *synchronously*
    /// (the value is stored in memory because it is read often by all services)
    fn get_ilp_address(&self) -> Address;
}

// Even though we wrap the types _a lot_ of times in multiple configurations
// the tests still build nearly instantly. The trick is to make the wrapping function
// take a trait object instead of the concrete type
#[cfg(test)]
mod tests {
    use super::*;
    use once_cell::sync::Lazy;
    use std::str::FromStr;

    #[test]
    fn incoming_service_no_exponential_blowup_when_wrapping() {
        // a normal async function
        async fn foo<A: Account>(
            request: IncomingRequest<A>,
            mut next: Box<dyn IncomingService<A> + Send>,
        ) -> IlpResult {
            next.handle_request(request).await
        }

        // and with a closure (async closure are unstable)
        let foo2 = move |request, mut next: Box<dyn IncomingService<TestAccount> + Send>| async move {
            next.handle_request(request).await
        };

        // base layer
        let s = BaseService;
        // our first layer
        let s: LayeredService<_, TestAccount> = LayeredService::new_incoming(s);

        // wrapped in our closure
        let s = WrappedService::wrap_incoming(s, foo2);
        let s = WrappedService::wrap_incoming(s, foo2);
        let s = WrappedService::wrap_incoming(s, foo2);
        let s = WrappedService::wrap_incoming(s, foo2);
        let s = WrappedService::wrap_incoming(s, foo2);

        // wrap it again in the normal service
        let s = LayeredService::new_incoming(s);

        // some short syntax
        let s = s.wrap(foo2);
        let s = s.wrap(foo);

        // called with the full syntax
        let s = WrappedService::wrap_incoming(s, foo);
        let s = WrappedService::wrap_incoming(s, foo);
        let s = WrappedService::wrap_incoming(s, foo);
        let s = WrappedService::wrap_incoming(s, foo);
        let s = WrappedService::wrap_incoming(s, foo);

        // more short syntax
        let s = s.wrap(foo2);
        let s = s.wrap(foo2);
        let _s = s.wrap(foo2);
    }

    #[test]
    fn outgoing_service_no_exponential_blowup_when_wrapping() {
        // a normal async function
        async fn foo<A: Account>(
            request: OutgoingRequest<A>,
            mut next: Box<dyn OutgoingService<A> + Send>,
        ) -> IlpResult {
            next.send_request(request).await
        }

        // and with a closure (async closure are unstable)
        let foo2 = move |request, mut next: Box<dyn OutgoingService<TestAccount> + Send>| async move {
            next.send_request(request).await
        };

        // base layer
        let s = BaseService;
        // our first layer
        let s: LayeredService<_, TestAccount> = LayeredService::new_outgoing(s);

        // wrapped in our closure
        let s = WrappedService::wrap_outgoing(s, foo2);
        let s = WrappedService::wrap_outgoing(s, foo2);
        let s = WrappedService::wrap_outgoing(s, foo2);
        let s = WrappedService::wrap_outgoing(s, foo2);
        let s = WrappedService::wrap_outgoing(s, foo2);

        // wrap it again in the normal service
        let s = LayeredService::new_outgoing(s);

        // some short syntax
        let s = s.wrap(foo2);
        let s = s.wrap(foo);

        // called with the full syntax
        let s = WrappedService::wrap_outgoing(s, foo);
        let s = WrappedService::wrap_outgoing(s, foo);
        let s = WrappedService::wrap_outgoing(s, foo);
        let s = WrappedService::wrap_outgoing(s, foo);
        let s = WrappedService::wrap_outgoing(s, foo);

        // more short syntax
        let s = s.wrap(foo2);
        let s = s.wrap(foo2);
        let _s = s.wrap(foo2);
    }

    #[derive(Clone)]
    struct BaseService;

    #[derive(Clone)]
    struct LayeredService<I, A> {
        next: I,
        account_type: PhantomData<A>,
    }

    impl<I, A> LayeredService<I, A>
    where
        I: IncomingService<A> + Send + Sync + 'static,
        A: Account,
    {
        fn new_incoming(next: I) -> Self {
            Self {
                next,
                account_type: PhantomData,
            }
        }
    }

    impl<I, A> LayeredService<I, A>
    where
        I: OutgoingService<A> + Send + Sync + 'static,
        A: Account,
    {
        fn new_outgoing(next: I) -> Self {
            Self {
                next,
                account_type: PhantomData,
            }
        }
    }

    #[async_trait]
    impl<A: Account + 'static> OutgoingService<A> for BaseService {
        async fn send_request(&mut self, _request: OutgoingRequest<A>) -> IlpResult {
            unimplemented!()
        }
    }

    #[async_trait]
    impl<A: Account + 'static> IncomingService<A> for BaseService {
        async fn handle_request(&mut self, _request: IncomingRequest<A>) -> IlpResult {
            unimplemented!()
        }
    }

    #[async_trait]
    impl<I, A> OutgoingService<A> for LayeredService<I, A>
    where
        I: OutgoingService<A> + Send + Sync + 'static,
        A: Account + Send + Sync + 'static,
    {
        async fn send_request(&mut self, _request: OutgoingRequest<A>) -> IlpResult {
            unimplemented!()
        }
    }

    #[async_trait]
    impl<I, A> IncomingService<A> for LayeredService<I, A>
    where
        I: IncomingService<A> + Send + Sync + 'static,
        A: Account + Send + Sync + 'static,
    {
        async fn handle_request(&mut self, _request: IncomingRequest<A>) -> IlpResult {
            unimplemented!()
        }
    }

    // Account test helpers
    #[derive(Clone, Debug)]
    pub struct TestAccount;

    pub static ALICE: Lazy<Username> = Lazy::new(|| Username::from_str("alice").unwrap());
    pub static EXAMPLE_ADDRESS: Lazy<Address> =
        Lazy::new(|| Address::from_str("example.alice").unwrap());

    impl Account for TestAccount {
        fn id(&self) -> Uuid {
            unimplemented!()
        }

        fn username(&self) -> &Username {
            &ALICE
        }

        fn asset_scale(&self) -> u8 {
            9
        }

        fn asset_code(&self) -> &str {
            "XYZ"
        }

        fn ilp_address(&self) -> &Address {
            &EXAMPLE_ADDRESS
        }
    }
}
