use futures::Future;
use interledger_packet::{ErrorCode, MaxPacketAmountDetails, RejectBuilder};
use interledger_service::*;

pub trait MaxPacketAmountStore {
    fn get_max_packet_amount(
        &self,
        account_id: AccountId,
    ) -> Box<Future<Item = u64, Error = ()> + Send>;
}

pub struct MaxPacketAmountService<S, T> {
    next: S,
    store: T,
}

impl<S, T> IncomingService for MaxPacketAmountService<S, T>
where
    // TODO does next need to have all of these?
    S: IncomingService + Send + Clone + 'static,
    T: MaxPacketAmountStore + Send,
{
    type Future = BoxedIlpFuture;

    fn handle_request(&mut self, request: IncomingRequest) -> Self::Future {
        let mut next = self.next.clone();
        Box::new(
            self.store
                .get_max_packet_amount(request.from)
                .map_err(|_| {
                    RejectBuilder {
                        code: ErrorCode::F02_UNREACHABLE,
                        message: b"Cannot find account record",
                        triggered_by: &[],
                        data: &[],
                    }
                    .build()
                })
                .and_then(move |max_packet_amount| {
                    if request.prepare.amount() <= max_packet_amount {
                        Ok(request)
                    } else {
                        let details = MaxPacketAmountDetails::new(
                            request.prepare.amount(),
                            max_packet_amount,
                        )
                        .to_bytes();
                        Err(RejectBuilder {
                            code: ErrorCode::F08_AMOUNT_TOO_LARGE,
                            message: &[],
                            triggered_by: &[],
                            data: &details[..],
                        }
                        .build())
                    }
                })
                .and_then(move |request| next.handle_request(request)),
        )
    }
}
