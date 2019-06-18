use futures::Future;
use interledger_packet::{Address, ErrorCode, Fulfill, Reject, RejectBuilder};
use interledger_service::*;
use std::marker::PhantomData;
use tokio_executor::spawn;

pub trait BalanceStore: AccountStore {
    /// Fetch the current balance for the given account.
    fn get_balance(&self, account: Self::Account)
        -> Box<dyn Future<Item = i64, Error = ()> + Send>;

    fn update_balances_for_prepare(
        &self,
        from_account: Self::Account,
        incoming_amount: u64,
        to_account: Self::Account,
        outgoing_amount: u64,
    ) -> Box<dyn Future<Item = (), Error = ()> + Send>;

    fn update_balances_for_fulfill(
        &self,
        from_account: Self::Account,
        incoming_amount: u64,
        to_account: Self::Account,
        outgoing_amount: u64,
    ) -> Box<dyn Future<Item = (), Error = ()> + Send>;

    fn update_balances_for_reject(
        &self,
        from_account: Self::Account,
        incoming_amount: u64,
        to_account: Self::Account,
        outgoing_amount: u64,
    ) -> Box<dyn Future<Item = (), Error = ()> + Send>;
}

/// # Balance Service
///
/// Responsible for managing the balances of the account and the interaction with the Settlement Engine
///
/// Requires an `Account` and a `BalanceStore`
#[derive(Clone)]
pub struct BalanceService<S, O, A> {
    ilp_address: Address,
    store: S,
    next: O,
    account_type: PhantomData<A>,
}

impl<S, O, A> BalanceService<S, O, A>
where
    S: BalanceStore,
    O: OutgoingService<A>,
    A: Account,
{
    pub fn new(ilp_address: Address, store: S, next: O) -> Self {
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
    S: BalanceStore<Account = A> + Clone + Send + Sync + 'static,
    O: OutgoingService<A> + Send + Clone + 'static,
    A: Account + 'static,
{
    type Future = BoxedIlpFuture;

    /// On send message:
    /// 1. Calls `store.update_balances_for_prepare` with the prepare.
    /// If it fails, it replies with a reject
    /// 1. Tries to forward the request:
    ///     - If it returns a fullfil, calls `store.update_balances_for_fulfill` and replies with the fulfill
    ///       INDEPENDENTLY of if the call suceeds or fails. This makes a `sendMoney` call if the fulfill puts the account's balance over the `settle_threshold`
    ///     - if it returns an reject calls `store.update_balances_for_reject` and replies with the fulfill
    ///       INDEPENDENTLY of if the call suceeds or fails
    fn send_request(
        &mut self,
        request: OutgoingRequest<A>,
    ) -> Box<dyn Future<Item = Fulfill, Error = Reject> + Send> {
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
                .update_balances_for_prepare(
                    from.clone(),
                    incoming_amount,
                    to.clone(),
                    outgoing_amount,
                )
                .map_err(move |_| {
                    debug!("Rejecting packet because it would exceed a balance limit");
                    RejectBuilder {
                        code: ErrorCode::T04_INSUFFICIENT_LIQUIDITY,
                        message: &[],
                        triggered_by: Some(&ilp_address),
                        data: &[],
                    }
                    .build()
                })
                .and_then(move |_| {
                    next.send_request(request)
                        .and_then(move |fulfill| {
                            // We will spawn a task to update the balances in the database
                            // so that we DO NOT wait for the database before sending the
                            // Fulfill packet back to our peer. Due to how the flow of ILP
                            // packets work, once we get the Fulfill back from the next node
                            // we need to propagate it backwards ASAP. If we do not give the
                            // previous node the fulfillment in time, they won't pay us back
                            // for the packet we forwarded. Note this means that we will
                            // relay the fulfillment _even if saving to the DB fails._
                            let fulfill_balance_update = store.update_balances_for_fulfill(
                                from.clone(),
                                incoming_amount,
                                to.clone(),
                                outgoing_amount,
                            ).map_err(move |_| error!("Error applying balance changes for fulfill from account: {} to account: {}. Incoming amount was: {}, outgoing amount was: {}", from.id(), to.id(), incoming_amount, outgoing_amount));
                            spawn(fulfill_balance_update);

                            Ok(fulfill)
                        })
                        .or_else(move |reject| {
                            // Similar to the logic for handling the Fulfill packet above, we
                            // spawn a task to update the balance for the Reject in parallel
                            // rather than waiting for the database to update before relaying
                            // the packet back. In this case, the only substantive difference
                            // would come from if the DB operation fails or takes too long.
                            // The packet is already rejected so it's more useful for the sender
                            // to get the error message from the original Reject packet rather
                            // than a less specific one saying that this node had an "internal
                            // error" caused by a database issue.
                            let reject_balance_update = store_clone.update_balances_for_reject(
                                from_clone.clone(),
                                incoming_amount,
                                to_clone.clone(),
                                outgoing_amount,
                            ).map_err(move |_| error!("Error rolling back balance change for accounts: {} and {}. Incoming amount was: {}, outgoing amount was: {}", from_clone.id(), to_clone.id(), incoming_amount, outgoing_amount));
                            spawn(reject_balance_update);

                            Err(reject)
                        })
                }),
        )
    }
}
