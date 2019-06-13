use futures::future::err;
use interledger_packet::{ErrorCode, MaxPacketAmountDetails, RejectBuilder};
use interledger_service::*;

pub trait MaxPacketAmountAccount: Account {
    fn max_packet_amount(&self) -> u64;
}

#[derive(Clone)]
pub struct MaxPacketAmountService<S> {
    next: S,
}

impl<S> MaxPacketAmountService<S> {
    pub fn new(next: S) -> Self {
        MaxPacketAmountService { next }
    }
}

impl<S, A> IncomingService<A> for MaxPacketAmountService<S>
where
    S: IncomingService<A>,
    A: MaxPacketAmountAccount,
{
    type Future = BoxedIlpFuture;

    fn handle_request(&mut self, request: IncomingRequest<A>) -> Self::Future {
        let max_packet_amount = request.from.max_packet_amount();
        if request.prepare.amount() <= max_packet_amount {
            Box::new(self.next.handle_request(request))
        } else {
            let details =
                MaxPacketAmountDetails::new(request.prepare.amount(), max_packet_amount).to_bytes();
            Box::new(err(RejectBuilder {
                code: ErrorCode::F08_AMOUNT_TOO_LARGE,
                message: &[],
                triggered_by: &[],
                data: &details[..],
            }
            .build()))
        }
    }
}
