use super::packet::*;
use super::Account;
use futures::future::ok;
use interledger_packet::*;
use interledger_service::*;
use log::debug;
use std::marker::PhantomData;

/// A simple service that intercepts incoming ILDCP requests
/// and responds using the information in the Account struct.
#[derive(Clone)]
pub struct IldcpService<I, A> {
    next: I,
    account_type: PhantomData<A>,
}

impl<I, A> IldcpService<I, A>
where
    I: IncomingService<A>,
    A: Account,
{
    pub fn new(next: I) -> Self {
        IldcpService {
            next,
            account_type: PhantomData,
        }
    }
}

impl<I, A> IncomingService<A> for IldcpService<I, A>
where
    I: IncomingService<A>,
    A: Account,
{
    type Future = BoxedIlpFuture;

    fn handle_request(&mut self, request: IncomingRequest<A>) -> Self::Future {
        if is_ildcp_request(&request.prepare) {
            let from = request.from.ilp_address();
            let builder = IldcpResponseBuilder {
                ilp_address: &from,
                asset_code: request.from.asset_code(),
                asset_scale: request.from.asset_scale(),
            };
            debug!("Responding to query for ildcp info by account: {:?}", from);
            let response = builder.build();
            let fulfill = Fulfill::from(response);
            Box::new(ok(fulfill))
        } else {
            Box::new(self.next.handle_request(request))
        }
    }
}
