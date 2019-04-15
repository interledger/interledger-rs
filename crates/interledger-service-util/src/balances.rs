use bytes::Bytes;
use futures::Future;
use interledger_ildcp::IldcpAccount;
use interledger_packet::{ErrorCode, Fulfill, Reject, RejectBuilder};
use interledger_service::*;
use std::marker::PhantomData;

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

#[derive(Clone)]
pub struct BalanceService<S, T, A> {
    ilp_address: Bytes,
    next: S,
    store: T,
    account_type: PhantomData<A>,
}

impl<S, T, A> BalanceService<S, T, A>
where
    S: OutgoingService<A>,
    T: BalanceStore,
    A: IldcpAccount + Account,
{
    pub fn new(ilp_address: Bytes, store: T, next: S) -> Self {
        BalanceService {
            ilp_address,
            next,
            store,
            account_type: PhantomData,
        }
    }
}

impl<S, T, A> OutgoingService<A> for BalanceService<S, T, A>
where
    // TODO can we make these non-'static?
    S: OutgoingService<A> + Send + Clone + 'static,
    T: BalanceStore<Account = A> + Clone + Send + Sync + 'static,
    A: IldcpAccount + Send + Sync + 'static,
{
    type Future = BoxedIlpFuture;

    fn send_request(
        &mut self,
        request: OutgoingRequest<A>,
    ) -> Box<Future<Item = Fulfill, Error = Reject> + Send> {
        let mut next = self.next.clone();
        let store = self.store.clone();
        let from = request.from.clone();
        let to = request.to.clone();
        let incoming_amount = request.original_amount;
        let outgoing_amount = request.prepare.amount();
        let ilp_address = self.ilp_address.clone();

        Box::new(
            self.store
                .update_balances(from.clone(), incoming_amount, to.clone(), outgoing_amount)
                .map_err(move |_| {
                    debug!("Rejecting packet because it would exceed a balance limit");
                    RejectBuilder {
                        code: ErrorCode::T04_INSUFFICIENT_LIQUIDITY,
                        message: &[],
                        triggered_by: &ilp_address,
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
