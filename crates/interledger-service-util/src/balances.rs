use bytes::Bytes;
use futures::Future;
use interledger_ildcp::IldcpAccount;
use interledger_packet::{ErrorCode, Fulfill, Reject, RejectBuilder};
use interledger_service::*;
use std::marker::PhantomData;

pub trait BalanceStore: AccountStore {
    /// Fetch the current balance for the given account.
    fn get_balance(&self, account: Self::Account) -> Box<Future<Item = i64, Error = ()> + Send>;

    fn update_balances_for_prepare(
        &self,
        from_account: Self::Account,
        incoming_amount: u64,
        to_account: Self::Account,
        outgoing_amount: u64,
    ) -> Box<Future<Item = (), Error = ()> + Send>;

    fn update_balances_for_fulfill(
        &self,
        from_account: Self::Account,
        incoming_amount: u64,
        to_account: Self::Account,
        outgoing_amount: u64,
    ) -> Box<Future<Item = (), Error = ()> + Send>;

    fn update_balances_for_reject(
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
        let store_clone = store.clone();
        let from = request.from.clone();
        let from_clone = from.clone();
        let to = request.to.clone();
        let to_clone = to.clone();
        let incoming_amount = request.original_amount;
        let outgoing_amount = request.prepare.amount();
        let ilp_address = self.ilp_address.clone();

        Box::new(
            self.store
                .update_balances_for_prepare(from.clone(), incoming_amount, to.clone(), outgoing_amount)
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
                        .and_then(move |fulfill| {
                            store.update_balances_for_fulfill(from.clone(), incoming_amount, to.clone(), outgoing_amount)
                                .then(move |result| {
                                    if result.is_err() {
                                        error!("Error applying balance changes for fulfill from account: {} to account: {}. Incoming amount was: {}, outgoing amount was: {}", from.id(), to.id(), incoming_amount, outgoing_amount);
                                    }
                                    Ok(fulfill)
                                })
                        })
                        .or_else(move |err| {
                            store_clone.update_balances_for_reject(from_clone.clone(), incoming_amount, to_clone.clone(), outgoing_amount)
                                .then(move |result| {
                                    if result.is_err() {
                                        error!("Error rolling back balance change for accounts: {} and {}. Incoming amount was: {}, outgoing amount was: {}", from_clone.id(), to_clone.id(), incoming_amount, outgoing_amount);
                                    }
                                    Err(err)
                                })
                        })
                }),
        )
    }
}
