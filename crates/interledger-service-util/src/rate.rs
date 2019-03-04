use futures::{future::err, Future};
use interledger_ildcp::IldcpAccount;
use interledger_packet::{ErrorCode, Fulfill, Reject, RejectBuilder};
use interledger_service::*;
use std::marker::PhantomData;
use std::sync::Arc;

pub trait BalanceStore: AccountStore {
    fn get_balance(&self, account: &Self::Account) -> Box<Future<Item = u64, Error = ()> + Send>;
    fn update_balances(
        &self,
        from_account: &Self::Account,
        incoming_amount: u64,
        to_account: &Self::Account,
        outgoing_amount: u64,
    ) -> Box<Future<Item = (), Error = ()> + Send>;
}

pub trait ExchangeRateStore {
    fn get_exchange_rates(&self, asset_codes: &[&str]) -> Result<Vec<f64>, ()>;
}

#[derive(Clone)]
pub struct ExchangeRateAndBalanceService<S, T> {
    next: S,
    store: T,
}

// TODO allow ExchangeRateStore and BalanceStore to be separate objects passed into the constructor
impl<S, T> ExchangeRateAndBalanceService<S, T>
where
    S: OutgoingService<T::Account>,
    T: ExchangeRateStore + BalanceStore,
{
    pub fn new(store: T, next: S) -> Self {
        ExchangeRateAndBalanceService { next, store }
    }
}

impl<S, T> OutgoingService<T::Account> for ExchangeRateAndBalanceService<S, T>
where
    // TODO can we make these non-'static?
    S: OutgoingService<T::Account> + Send + Clone + 'static,
    T: BalanceStore + ExchangeRateStore + Send,
    T::Account: IldcpAccount + 'static,
{
    type Future = BoxedIlpFuture;

    fn send_request(
        &mut self,
        mut request: OutgoingRequest<<T as AccountStore>::Account>,
    ) -> Box<Future<Item = Fulfill, Error = Reject> + Send> {
        if let Ok(rates) = self
            .store
            .get_exchange_rates(&[&request.from.asset_code(), &request.to.asset_code()])
        {
            // TODO use bignums to make sure none of these numbers overflow
            let scale_change = u32::from(request.to.asset_scale() - request.from.asset_scale());
            let outgoing_amount =
                (rates[1] / rates[0] * request.prepare.amount().pow(scale_change) as f64) as u64;
            debug!("Converted incoming amount of {} {} (scale: {}) to outgoing amount of {} {} (scale: {})", request.prepare.amount(), request.from.asset_code(), request.from.asset_scale(), outgoing_amount, request.to.asset_code(), request.to.asset_scale());
            request.prepare.set_amount(outgoing_amount);

            let mut next = self.next.clone();
            Box::new(
                self.store
                    .update_balances(
                        &request.from,
                        request.prepare.amount(),
                        &request.to,
                        outgoing_amount,
                    )
                    .map_err(|_| {
                        debug!("Rejecting packet because it would exceed a balance limit");
                        RejectBuilder {
                            code: ErrorCode::T04_INSUFFICIENT_LIQUIDITY,
                            message: &[],
                            triggered_by: &[],
                            data: &[],
                        }
                        .build()
                    })
                    .and_then(move |_| next.send_request(request)),
            )
        } else {
            error!(
                "Error getting exchange rates for assets: {}, {}",
                request.from.asset_code(),
                request.to.asset_code()
            );
            Box::new(err(RejectBuilder {
                code: ErrorCode::T00_INTERNAL_ERROR,
                message: &[],
                triggered_by: &[],
                data: &[],
            }
            .build()))
        }
    }
}
