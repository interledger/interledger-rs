use super::packet::*;
use super::IldcpAccount;
use futures::future::ok;
use interledger_packet::*;
use interledger_service::*;

pub struct IldcpService<S> {
    next: S,
}

impl<S, A> IncomingService<A> for IldcpService<S>
where
    S: IncomingService<A>,
    A: IldcpAccount,
{
    type Future = BoxedIlpFuture;

    fn handle_request(&mut self, request: IncomingRequest<A>) -> Self::Future {
        if is_ildcp_request(&request.prepare) {
            let builder = IldcpResponseBuilder {
                client_address: &request.from.client_address(),
                asset_code: &request.from.asset_code(),
                asset_scale: request.from.asset_scale(),
            };
            let response = builder.build();
            let fulfill = Fulfill::from(response);
            Box::new(ok(fulfill))
        } else {
            Box::new(self.next.handle_request(request))
        }
    }
}
