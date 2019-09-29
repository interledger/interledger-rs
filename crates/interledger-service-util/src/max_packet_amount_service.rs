use futures::future::err;
use interledger_packet::{ErrorCode, MaxPacketAmountDetails, RejectBuilder};
use interledger_service::*;
use log::debug;

pub trait MaxPacketAmountAccount: Account {
    fn max_packet_amount(&self) -> u64;
}

/// # MaxPacketAmount Service
///
/// This service is used by nodes to limit the maximum value of each packet they are willing to forward.
/// Nodes may limit the packet amount for a variety of reasons:
/// - Liquidity: a node operator may not way to allow a single high-value packet to tie up a large portion of its liquidity at once (especially because they do not know whether the packet will be fulfilled or rejected)
/// - Security: each packet carries some risk, due to the possibility that a node's failure to pass back the fulfillment within the available time window would cause that node to lose money. Keeping the value of each individual packet low may help reduce the impact of such a failure
/// Signaling: nodes SHOULD set the maximum packet amount _lower_ than the maximum amount in flight (also known as the payment or money bandwidth). `T04: Insufficient Liquidity` errors do not communicate to the sender how much they can send, largely because the "available liquidity" may be time based or based on the rate of other payments going through and thus difficult to communicate effectively. In contrast, the `F08: Amount Too Large` error conveys the maximum back to the sender, because this limit is assumed to be a static value, and alllows sender-side software like STREAM implementations to respond accordingly. Therefore, setting the maximum packet amount lower than the total money bandwidth allows client implementations to quickly adjust their packet amounts to appropriate levels.
/// Requires a `MaxPacketAmountAccount` and _no store_.
#[derive(Clone)]
pub struct MaxPacketAmountService<I, S> {
    next: I,
    store: S,
}

impl<I, S> MaxPacketAmountService<I, S> {
    pub fn new(store: S, next: I) -> Self {
        MaxPacketAmountService { store, next }
    }
}

impl<I, S, A> IncomingService<A> for MaxPacketAmountService<I, S>
where
    I: IncomingService<A>,
    S: AddressStore,
    A: MaxPacketAmountAccount,
{
    type Future = BoxedIlpFuture;

    /// On receive request:
    /// 1. if request.prepare.amount <= request.from.max_packet_amount forward the request, else error
    fn handle_request(&mut self, request: IncomingRequest<A>) -> Self::Future {
        let ilp_address = self.store.get_ilp_address();
        let max_packet_amount = request.from.max_packet_amount();
        if request.prepare.amount() <= max_packet_amount {
            Box::new(self.next.handle_request(request))
        } else {
            debug!(
                "Prepare amount:{} exceeds max_packet_amount: {}",
                request.prepare.amount(),
                max_packet_amount
            );
            let details =
                MaxPacketAmountDetails::new(request.prepare.amount(), max_packet_amount).to_bytes();
            Box::new(err(RejectBuilder {
                code: ErrorCode::F08_AMOUNT_TOO_LARGE,
                message: &[],
                triggered_by: Some(&ilp_address),
                data: &details[..],
            }
            .build()))
        }
    }
}
