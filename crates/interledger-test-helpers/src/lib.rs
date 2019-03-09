use bytes::Bytes;
use futures::future::{result, FutureResult};
use interledger_ildcp::IldcpAccount;
use interledger_packet::{Fulfill, Reject};
use interledger_service::*;
use parking_lot::Mutex;
use std::{marker::PhantomData, sync::Arc};

#[derive(Debug, Eq, PartialEq, Clone)]
pub struct TestAccount {
    id: u64,
    ilp_address: Bytes,
    asset_scale: u8,
    asset_code: String,
}

impl TestAccount {
    pub fn new(id: u64, ilp_address: &[u8], asset_code: &str, asset_scale: u8) -> Self {
        TestAccount {
            id,
            ilp_address: Bytes::from(ilp_address),
            asset_code: asset_code.to_string(),
            asset_scale,
        }
    }

    pub fn default() -> Self {
        TestAccount {
            id: 0,
            ilp_address: Bytes::from("example.account"),
            asset_code: "XYZ".to_string(),
            asset_scale: 9,
        }
    }
}

impl Account for TestAccount {
    type AccountId = u64;

    fn id(&self) -> u64 {
        self.id
    }
}

impl IldcpAccount for TestAccount {
    fn asset_code(&self) -> String {
        self.asset_code.clone()
    }

    fn asset_scale(&self) -> u8 {
        self.asset_scale
    }

    fn client_address(&self) -> Bytes {
        self.ilp_address.clone()
    }
}

#[derive(Clone)]
pub struct TestIncomingService<A: Account> {
    response: Result<Fulfill, Reject>,
    incoming_requests: Arc<Mutex<Vec<IncomingRequest<A>>>>,
    account_type: PhantomData<A>,
}

impl<A> TestIncomingService<A>
where
    A: Account,
{
    pub fn fulfill(fulfill: Fulfill) -> Self {
        TestIncomingService {
            response: Ok(fulfill),
            incoming_requests: Arc::new(Mutex::new(Vec::new())),
            account_type: PhantomData,
        }
    }

    pub fn reject(reject: Reject) -> Self {
        TestIncomingService {
            response: Err(reject),
            incoming_requests: Arc::new(Mutex::new(Vec::new())),
            account_type: PhantomData,
        }
    }

    pub fn get_incoming_requests(&self) -> Vec<IncomingRequest<A>> {
        self.incoming_requests.lock().clone()
    }
}

impl<A> IncomingService<A> for TestIncomingService<A>
where
    A: Account,
{
    type Future = FutureResult<Fulfill, Reject>;

    fn handle_request(&mut self, request: IncomingRequest<A>) -> Self::Future {
        self.incoming_requests.lock().push(request);
        result(self.response.clone())
    }
}

#[derive(Clone)]
pub struct TestOutgoingService<A: Account> {
    response: Result<Fulfill, Reject>,
    outgoing_requests: Arc<Mutex<Vec<OutgoingRequest<A>>>>,
    account_type: PhantomData<A>,
}

impl<A> TestOutgoingService<A>
where
    A: Account,
{
    pub fn fulfill(fulfill: Fulfill) -> Self {
        TestOutgoingService {
            response: Ok(fulfill),
            outgoing_requests: Arc::new(Mutex::new(Vec::new())),
            account_type: PhantomData,
        }
    }

    pub fn reject(reject: Reject) -> Self {
        TestOutgoingService {
            response: Err(reject),
            outgoing_requests: Arc::new(Mutex::new(Vec::new())),
            account_type: PhantomData,
        }
    }

    pub fn get_outgoing_requests(&self) -> Vec<OutgoingRequest<A>> {
        self.outgoing_requests.lock().clone()
    }
}

impl<A> OutgoingService<A> for TestOutgoingService<A>
where
    A: Account,
{
    type Future = FutureResult<Fulfill, Reject>;

    fn send_request(&mut self, request: OutgoingRequest<A>) -> Self::Future {
        self.outgoing_requests.lock().push(request);
        result(self.response.clone())
    }
}
