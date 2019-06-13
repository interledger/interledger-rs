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

/// # Balance Service
///
/// Responsible for managing the balances of the account and the interaction with the Settlement Engine
///
/// Requires an `IldcpAccount` and a `BalanceStore`
#[derive(Clone)]
pub struct BalanceService<S, O, A> {
    ilp_address: Bytes,
    store: S,
    next: O,
    account_type: PhantomData<A>,
}

impl<S, O, A> BalanceService<S, O, A>
where
    S: BalanceStore,
    O: OutgoingService<A>,
    A: IldcpAccount,
{
    pub fn new(ilp_address: Bytes, store: S, next: O) -> Self {
        BalanceService {
            ilp_address,
            store,
            next,
            account_type: PhantomData,
        }
    }
}

impl<S, O, A> OutgoingService<A> for BalanceService<S, O, A>
where
    // TODO can we make these non-'static?
    S: BalanceStore<Account = A> + Clone + Send + Sync + 'static,
    O: OutgoingService<A> + Send + Clone + 'static,
    A: IldcpAccount + Sync + 'static, // Should we just incorporate ILDCPAccount into Account?
{
    type Future = BoxedIlpFuture;

    /// On send message:
    /// 1. Calls `store.update_balances_for_prepare` with the prepare.
    /// If it fails, it replies with a reject
    /// 1. Tries to forward the request:
    ///     - If it returns a fullfil, calls `store.update_balances_for_fulfill` and replies with the fulfill
    ///       INDEPENDENTLY of if the call suceeds or fails. THIS MAKES A `sendMoney` CALL TO THE SETTLEMENT ENGINE
    ///     - if it returns an reject calls `store.update_balances_for_reject` and replies with the fulfill
    ///       INDEPENDENTLY of if the call suceeds or fails
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
                            // TODO should we spawn a task to update the balances instead of doing it before returning the fulfill?
                            // do we always return the fulfill, even if the state update fails? If so, I think we can.
                            store.update_balances_for_fulfill(from.clone(), incoming_amount, to.clone(), outgoing_amount)
                                .then(move |result| {
                                    if result.is_err() {
                                        error!("Error applying balance changes for fulfill from account: {} to account: {}. Incoming amount was: {}, outgoing amount was: {}", from.id(), to.id(), incoming_amount, outgoing_amount);
                                    }
                                    Ok(fulfill)
                                })
                        })
                        .or_else(move |reject| {
                            // can spawn here, if it is the case that the returned packet is not dependent on the outcome of the store state update
                            store_clone.update_balances_for_reject(from_clone.clone(), incoming_amount, to_clone.clone(), outgoing_amount)
                                .then(move |result| {
                                    if result.is_err() {
                                        error!("Error rolling back balance change for accounts: {} and {}. Incoming amount was: {}, outgoing amount was: {}", from_clone.id(), to_clone.id(), incoming_amount, outgoing_amount);
                                    }
                                    Err(reject)
                                })
                        })
                }),
        )
    }
}
