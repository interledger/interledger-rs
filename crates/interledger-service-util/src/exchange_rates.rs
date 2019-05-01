use bytes::Bytes;
use futures::{future::err, Future};
use interledger_ildcp::IldcpAccount;
use interledger_packet::{ErrorCode, Fulfill, Reject, RejectBuilder};
use interledger_service::*;
use std::marker::PhantomData;

pub trait ExchangeRateStore {
    fn get_exchange_rates(&self, asset_codes: &[&str]) -> Result<Vec<f64>, ()>;
}

#[derive(Clone)]
pub struct ExchangeRateService<S, T, A> {
    ilp_address: Bytes,
    next: S,
    store: T,
    account_type: PhantomData<A>,
}

impl<S, T, A> ExchangeRateService<S, T, A>
where
    S: OutgoingService<A>,
    T: ExchangeRateStore,
    A: IldcpAccount + Account,
{
    pub fn new(ilp_address: Bytes, store: T, next: S) -> Self {
        ExchangeRateService {
            ilp_address,
            next,
            store,
            account_type: PhantomData,
        }
    }
}

impl<S, T, A> OutgoingService<A> for ExchangeRateService<S, T, A>
where
    // TODO can we make these non-'static?
    S: OutgoingService<A> + Send + Clone + 'static,
    T: ExchangeRateStore + Clone + Send + Sync + 'static,
    A: IldcpAccount + Send + Sync + 'static,
{
    type Future = BoxedIlpFuture;

    fn send_request(
        &mut self,
        mut request: OutgoingRequest<A>,
    ) -> Box<Future<Item = Fulfill, Error = Reject> + Send> {
        if request.prepare.amount() > 0 {
            let rate: f64 = if request.from.asset_code() == request.to.asset_code() {
                1f64
            } else if let Ok(rates) = self
                .store
                .get_exchange_rates(&[&request.from.asset_code(), &request.to.asset_code()])
            {
                rates[1] / rates[0]
            } else {
                error!(
                    "No exchange rates available for assets: {}, {}",
                    request.from.asset_code(),
                    request.to.asset_code()
                );
                return Box::new(err(RejectBuilder {
                    code: ErrorCode::T00_INTERNAL_ERROR,
                    message: &[],
                    triggered_by: &self.ilp_address,
                    data: &[],
                }
                .build()));
            };

            let scaled_rate = if request.to.asset_scale() >= request.from.asset_scale() {
                rate * 10f64.powf(f64::from(request.to.asset_scale() - request.from.asset_scale()))
            } else {
                rate / 10f64.powf(f64::from(request.from.asset_scale() - request.to.asset_scale()))
            };

            let outgoing_amount = (request.prepare.amount() as f64 * scaled_rate) as u64;
            request.prepare.set_amount(outgoing_amount);
            debug!("Converted incoming amount of: {} {} (scale {}) from account {} to outgoing amount of: {} {} (scale {}) for account {}", request.original_amount, request.from.asset_code(), request.from.asset_scale(), request.from.id(), outgoing_amount, request.to.asset_code(), request.to.asset_scale(), request.to.id());
        }

        Box::new(self.next.send_request(request))
    }
}
