use futures::{future::err, Future};
use interledger_packet::{ErrorCode, Fulfill, Reject, RejectBuilder};
use interledger_service::*;

pub trait BalanceStore: AccountStore {
    /// Fetch the current balance for the given account.
    fn get_balance(&self, account: Self::Account) -> Box<Future<Item = i64, Error = ()> + Send>;

    /// Subtract the `incoming_amount` from the `from_account`'s balance.
    /// Add the `outgoing_amount` to the `to_account`'s balance.
    fn update_balances(
        &self,
        from_account: Self::Account,
        incoming_amount: u64,
        to_account: Self::Account,
        outgoing_amount: u64,
    ) -> Box<Future<Item = (), Error = ()> + Send>;

    /// Roll back the effect of a previous `update_balances` call.
    /// Add the `incoming_amount` to the `from_account`'s balance.
    /// Subtract the `outgoing_amount` from the `to_account`'s balance.
    fn undo_balance_update(
        &self,
        from_account: Self::Account,
        incoming_amount: u64,
        to_account: Self::Account,
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
    T: BalanceStore + ExchangeRateStore + Clone + Send + Sync + 'static,
    T::Account: Account + Send + Sync + 'static,
{
    type Future = BoxedIlpFuture;

    fn send_request(
        &mut self,
        mut request: OutgoingRequest<<T as AccountStore>::Account>,
    ) -> Box<Future<Item = Fulfill, Error = Reject> + Send> {
        let scale_change = u32::from(request.to.asset_scale() - request.from.asset_scale());
        let outgoing_amount = if request.from.asset_code() == request.to.asset_code() {
            debug!("Same currency. Forwarding request.");
            request.prepare.amount() * 10u64.pow(scale_change)
        } else if let Ok(rates) = self
            .store
            .get_exchange_rates(&[&request.from.asset_code(), &request.to.asset_code()])
        {
            // TODO use bignums to make sure none of these numbers overflow
            let outgoing_amount = (rates[1] / rates[0]
                * request.prepare.amount() as f64
                * 10u64.pow(scale_change) as f64) as u64;
            debug!("Converted incoming amount of {} {} (scale: {}) to outgoing amount of {} {} (scale: {})", request.prepare.amount(), request.from.asset_code(), request.from.asset_scale(), outgoing_amount, request.to.asset_code(), request.to.asset_scale());
            outgoing_amount
        } else {
            error!(
                "Error getting exchange rates for assets: {}, {}",
                request.from.asset_code(),
                request.to.asset_code()
            );
            return Box::new(err(RejectBuilder {
                code: ErrorCode::T00_INTERNAL_ERROR,
                message: &[],
                triggered_by: None,
                data: &[],
            }
            .build()));
        };

        let mut next = self.next.clone();
        let store = self.store.clone();
        let from = request.from.clone();
        let to = request.to.clone();
        let incoming_amount = request.prepare.amount();

        request.prepare.set_amount(outgoing_amount);
        Box::new(
            self.store
                .update_balances(from.clone(), incoming_amount, to.clone(), outgoing_amount)
                .map_err(|_| {
                    debug!("Rejecting packet because it would exceed a balance limit");
                    RejectBuilder {
                        code: ErrorCode::T04_INSUFFICIENT_LIQUIDITY,
                        message: &[],
                        triggered_by: None,
                        data: &[],
                    }
                    .build()
                })
                .and_then(move |_| {
                    next.send_request(request)
                        .or_else(move |err| store.undo_balance_update(from.clone(), incoming_amount, to.clone(), outgoing_amount)
                        .then(move |result| {
                            if result.is_err() {
                                error!("Error rolling back balance change for accounts: {} and {}. Incoming amount was: {}, outgoing amount was: {}", from.id(), to.id(), incoming_amount, outgoing_amount);
                            }
                            Err(err)
                        }))
                }),
        )
    }
}
