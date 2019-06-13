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
pub struct ExchangeRateService<S, O, A> {
    ilp_address: Bytes,
    store: S,
    next: O,
    account_type: PhantomData<A>,
}

impl<S, O, A> ExchangeRateService<S, O, A>
where
    S: ExchangeRateStore,
    O: OutgoingService<A>,
    A: IldcpAccount,
{
    pub fn new(ilp_address: Bytes, store: S, next: O) -> Self {
        ExchangeRateService {
            ilp_address,
            store,
            next,
            account_type: PhantomData,
        }
    }
}

impl<S, O, A> OutgoingService<A> for ExchangeRateService<S, O, A>
where
    // TODO can we make these non-'static?
    S: ExchangeRateStore + Clone + Send + Sync + 'static,
    O: OutgoingService<A> + Send + Clone + 'static,
    A: IldcpAccount + Sync + 'static,
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
                    // Unreachable doesn't seem to be the correct code here.
                    // If the pair was not found, shouldn't we have a unique error code
                    // for that such as `ErrorCode::F10_PAIRNOTFOUND` ?
                    // Timeout should still apply we if we add a timeout
                    // error in the get_exchange_rate call
                    code: ErrorCode::F02_UNREACHABLE,
                    message: format!(
                        "No exchange rate available from asset: {} to: {}",
                        request.from.asset_code(),
                        request.to.asset_code()
                    )
                    .as_bytes()
                    .as_ref(),
                    triggered_by: &self.ilp_address,
                    data: &[],
                }
                .build()));
            };

            let scaled_rate = if request.to.asset_scale() >= request.from.asset_scale() {
                rate * 10f64.powf(f64::from(
                    request.to.asset_scale() - request.from.asset_scale(),
                ))
            } else {
                rate / 10f64.powf(f64::from(
                    request.from.asset_scale() - request.to.asset_scale(),
                ))
            };

            let outgoing_amount = (request.prepare.amount() as f64 * scaled_rate) as u64;
            request.prepare.set_amount(outgoing_amount);
            trace!("Converted incoming amount of: {} {} (scale {}) from account {} to outgoing amount of: {} {} (scale {}) for account {}", request.original_amount, request.from.asset_code(), request.from.asset_scale(), request.from.id(), outgoing_amount, request.to.asset_code(), request.to.asset_scale(), request.to.id());
        }

        Box::new(self.next.send_request(request))
    }
}