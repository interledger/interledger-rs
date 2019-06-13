use futures::future::err;
use interledger_packet::{ErrorCode, MaxPacketAmountDetails, RejectBuilder};
use interledger_service::*;

pub trait MaxPacketAmountAccount: Account {
    fn max_packet_amount(&self) -> u64;
}

/// # MaxPacketAmount Service
///
/// Used by the connector to limit the maximum size of each packet they want to forward. They may want to limit that size for liquidity or security reasons (you might not want one big packet using up a bunch of liquidity at once and since each packet carries some risk because of the different timeouts, you might want to keep each individual packet relatively small)
/// Requires a `MaxPacketAmountAccount` and _no store_.
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

    /// On receive request:
    /// 1. if request.prepare.amount <= request.from.max_packet_amount forward the request, else error
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
