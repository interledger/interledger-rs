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
pub struct IldcpService<I> {
    next: I,
}

impl<I> IldcpService<I>
where
    I: IncomingService,
{
    pub fn new(next: I) -> Self {
        IldcpService { next }
    }
}

impl<I> IncomingService for IldcpService<I>
where
    I: IncomingService,
{
    type Future = BoxedIlpFuture;

    fn handle_request(&mut self, request: IncomingRequest) -> Self::Future {
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
