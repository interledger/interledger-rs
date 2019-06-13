use futures::future::err;
use interledger_packet::{ErrorCode, MaxPacketAmountDetails, RejectBuilder};
use interledger_service::*;

pub trait MaxPacketAmountAccount: Account {
    fn max_packet_amount(&self) -> u64;
}

#[derive(Clone)]
pub struct MaxPacketAmountService<I> {
    next: I,
}

impl<I> MaxPacketAmountService<I> {
    pub fn new(next: I) -> Self {
        MaxPacketAmountService { next }
    }
}

impl<I, A> IncomingService<A> for MaxPacketAmountService<I>
where
    I: IncomingService<A>,
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
