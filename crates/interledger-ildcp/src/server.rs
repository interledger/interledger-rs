use super::packet::*;
use super::IldcpAccount;
use futures::future::ok;
use interledger_packet::*;
use interledger_service::*;
use std::{marker::PhantomData, str};

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
    A: IldcpAccount,
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
    A: IldcpAccount,
{
    type Future = BoxedIlpFuture;

    fn handle_request(&mut self, request: IncomingRequest<A>) -> Self::Future {
        if is_ildcp_request(&request.prepare) {
            let builder = IldcpResponseBuilder {
                client_address: &request.from.client_address(),
                asset_code: request.from.asset_code(),
                asset_scale: request.from.asset_scale(),
            };
            trace!(
                "Responding to query for ILDCP info by account: {:?}",
                str::from_utf8(&request.from.client_address()[..]).unwrap_or("<not utf8>")
            );
            let response = builder.build();
            let fulfill = Fulfill::from(response);
            Box::new(ok(fulfill))
        } else {
            Box::new(self.next.handle_request(request))
        }
    }
}