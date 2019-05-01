use super::packet::*;
use super::IldcpAccount;
use futures::future::ok;
use interledger_packet::*;
use interledger_service::*;
use std::{marker::PhantomData, str};

/// A simple service that intercepts incoming ILDCP requests
/// and responds using the information in the Account struct.
#[derive(Clone)]
pub struct IldcpService<S, A> {
    next: S,
    account_type: PhantomData<A>,
}

impl<S, A> IldcpService<S, A>
where
    S: IncomingService<A>,
    A: IldcpAccount,
{
    pub fn new(next: S) -> Self {
        IldcpService {
            next,
            account_type: PhantomData,
        }
    }
}

impl<S, A> IncomingService<A> for IldcpService<S, A>
where
    S: IncomingService<A>,
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
