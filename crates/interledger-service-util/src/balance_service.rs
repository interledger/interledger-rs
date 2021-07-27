use async_trait::async_trait;
use futures::TryFutureExt;
use interledger_errors::BalanceStoreError;
use interledger_packet::{ErrorCode, RejectBuilder};
use interledger_service::*;
use interledger_settlement::core::{
    types::{SettlementAccount, SettlementStore},
    SettlementClient,
};
use std::marker::PhantomData;
use std::sync::{Arc, Mutex};
use std::{fmt, time::Duration, time::Instant};
use tokio::sync::mpsc::error::TrySendError;
use tracing::{debug, error, info, trace, warn};
use uuid::Uuid;

// TODO: Remove AccountStore dependency, use `AccountId: ToString` as associated type
/// Trait responsible for managing an account's balance in the store
/// as ILP Packets get routed
#[async_trait]
pub trait BalanceStore {
    /// Fetch the current balance for the given account id.
    async fn get_balance(&self, account_id: Uuid) -> Result<i64, BalanceStoreError>;

    /// Decreases the sending account's balance before forwarding out a prepare packet
    async fn update_balances_for_prepare(
        &self,
        from_account_id: Uuid,
        incoming_amount: u64,
    ) -> Result<(), BalanceStoreError>;

    /// Increases the receiving account's balance, and returns the updated balance
    /// along with the amount which should be settled
    async fn update_balances_for_fulfill(
        &self,
        to_account_id: Uuid,
        outgoing_amount: u64,
    ) -> Result<(i64, u64), BalanceStoreError>;

    async fn update_balances_for_reject(
        &self,
        from_account_id: Uuid,
        incoming_amount: u64,
    ) -> Result<(), BalanceStoreError>;

    /// Removes any positive amount to settle over `settle_to` from `balance`. Similarly to other
    /// balance updates once this call succeeds the amount needed to be settled is only in the
    /// interledger node so crashing while the HTTP request to settlement-engine hasn't gone
    /// through or this balance hasn't been refunded will lead to loss of the "amount to settle".
    ///
    /// Returns (balance, amount_to_settle).
    async fn update_balances_for_delayed_settlement(
        &self,
        to_account_id: Uuid,
    ) -> Result<(i64, u64), BalanceStoreError>;
}

/// # Balance Service
///
/// Responsible for managing the balances of the account and the interaction with the Settlement Engine
///
/// Requires an `Account` and a `BalanceStore`
#[derive(Clone)]
pub struct BalanceService<S, O, A> {
    store: S,
    next: O,
    settlement_client: SettlementClient,
    policy: Policy,
    account_type: PhantomData<A>,
    channel_last_fail: Arc<Mutex<Instant>>,
}

impl<S, O, A> BalanceService<S, O, A>
where
    S: AddressStore + BalanceStore + SettlementStore<Account = A>,
    O: OutgoingService<A>,
    A: Account + SettlementAccount,
{
    pub fn new(
        store: S,
        sender: Option<tokio::sync::mpsc::Sender<ManageTimeout>>,
        next: O,
    ) -> Self {
        BalanceService {
            store,
            next,
            settlement_client: SettlementClient::default(),
            policy: match sender {
                Some(tx) => Policy::TimeBased(tx),
                None => Policy::ThresholdOnly,
            },
            account_type: PhantomData,
            channel_last_fail: Arc::new(Mutex::new(Instant::now())),
        }
    }
}

#[async_trait]
impl<S, O, A> OutgoingService<A> for BalanceService<S, O, A>
where
    S: AddressStore + BalanceStore + SettlementStore<Account = A> + Clone + Send + Sync + 'static,
    O: OutgoingService<A> + Send + Clone + 'static,
    A: SettlementAccount + Send + Sync + 'static,
{
    /// On send message:
    /// 1. Calls `store.update_balances_for_prepare` with the prepare.
    /// If it fails, it replies with a reject
    /// 1. Tries to forward the request:
    ///     - If it returns a fullfil, calls `store.update_balances_for_fulfill` and replies with the fulfill
    ///       INDEPENDENTLY of if the call suceeds or fails. This makes a `sendMoney` call if the fulfill puts the account's balance over the `settle_threshold`
    ///     - if it returns an reject calls `store.update_balances_for_reject` and replies with the fulfill
    ///       INDEPENDENTLY of if the call suceeds or fails
    async fn send_request(&mut self, request: OutgoingRequest<A>) -> IlpResult {
        // Don't bother touching the store for zero-amount packets.
        // Note that it is possible for the original_amount to be >0 while the
        // prepare.amount is 0, because the original amount could be rounded down
        // to 0 when exchange rate and scale change are applied.
        if request.prepare.amount() == 0 && request.original_amount == 0 {
            // wonder if timeout should still be set here?
            return self.next.send_request(request).await;
        }

        let mut next = self.next.clone();
        let store = self.store.clone();
        let from = request.from.clone();
        let from_clone = from.clone();
        let from_id = from.id();
        let to = request.to.clone();
        let to_clone = to.clone();
        let incoming_amount = request.original_amount;
        let outgoing_amount = request.prepare.amount();
        let ilp_address = self.store.get_ilp_address();
        let settlement_client = self.settlement_client.clone();

        // Update the balance _before_ sending the settlement so that we don't accidentally send
        // multiple settlements for the same balance. While there will be a small moment of time (the delta
        // between this balance change and the moment that the settlement-engine accepts the request for
        // settlement payment) where the actual balance in Redis is less than it should be, this is tolerable
        // because this amount of time will always be small. This is because the design of the settlement
        // engine API is asynchronous, meaning when a request is made to the settlement engine, it will
        // accept the request and return (milliseconds) with a guarantee that the settlement payment will
        //  _eventually_ be completed. Because of this settlement_engine guarantee, the Connector can
        // operate as-if the settlement engine has completed. Finally, if the request to the settlement-engine
        // fails, this amount will be re-added back to balance.
        self.store
            .update_balances_for_prepare(from_id, incoming_amount)
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
            .await?;

        match next.send_request(request).await {
            Ok(fulfill) => {
                if outgoing_amount > 0 {
                    // We will spawn a task to update the balances in the database
                    // so that we DO NOT wait for the database before sending the
                    // Fulfill packet back to our peer. Due to how the flow of ILP
                    // packets work, once we get the Fulfill back from the next node
                    // we need to propagate it backwards ASAP. If we do not give the
                    // previous node the fulfillment in time, they won't pay us back
                    // for the packet we forwarded. Note this means that we will
                    // relay the fulfillment _even if saving to the DB fails._
                    settle_or_rollback_later(
                        incoming_amount,
                        outgoing_amount,
                        store,
                        from_id,
                        to,
                        settlement_client,
                        self.policy.clone(),
                        self.channel_last_fail.clone(),
                    );
                }

                Ok(fulfill)
            }
            Err(reject) => {
                // Similar to the logic for handling the Fulfill packet above, we
                // spawn a task to update the balance for the Reject in parallel
                // rather than waiting for the database to update before relaying
                // the packet back. In this case, the only substantive difference
                // would come from if the DB operation fails or takes too long.
                // The packet is already rejected so it's more useful for the sender
                // to get the error message from the original Reject packet rather
                // than a less specific one saying that this node had an "internal
                // error" caused by a database issue.
                tokio::spawn({
                    let store_clone = self.store.clone();
                    async move {
                        store_clone.update_balances_for_reject(
                            from_clone.id(),
                            incoming_amount,
                        ).map_err(move |_| error!("Error rolling back balance change for accounts: {} and {}. Incoming amount was: {}, outgoing amount was: {}", from_clone.id(), to_clone.id(), incoming_amount, outgoing_amount)).await
                    }
                });

                Err(reject)
            }
        }
    }
}

// See comments above in the BalanceStore::send_request why this is done in another task.
#[allow(clippy::too_many_arguments)]
fn settle_or_rollback_later<Acct, Store>(
    incoming_amount: u64,
    outgoing_amount: u64,
    store: Store,
    from_id: Uuid,
    to: Acct,
    settlement_client: SettlementClient,
    policy: Policy,
    channel_last_fail: Arc<Mutex<Instant>>,
) where
    Acct: SettlementAccount + Send + Sync + 'static,
    Store: BalanceStore + SettlementStore<Account = Acct> + Send + Sync + 'static,
{
    tokio::spawn(settle_or_rollback_now(
        incoming_amount,
        outgoing_amount,
        store,
        from_id,
        to,
        settlement_client,
        policy,
        channel_last_fail,
    ));
}

#[allow(clippy::too_many_arguments)]
async fn settle_or_rollback_now<Acct, Store>(
    incoming_amount: u64,
    outgoing_amount: u64,
    store: Store,
    from_id: Uuid,
    to: Acct,
    settlement_client: SettlementClient,
    mut policy: Policy,
    channel_last_fail: Arc<Mutex<Instant>>,
) -> Result<(), ()>
where
    Acct: SettlementAccount + Send + Sync + 'static,
    Store: BalanceStore + SettlementStore<Account = Acct> + Send + Sync + 'static,
{
    let (balance, amount_to_settle) = store
        .update_balances_for_fulfill(to.id(), outgoing_amount)
        .map_err(|err| error!("Error applying balance changes for fulfill from account: {} to account: {}. Incoming amount was: {}, outgoing amount was: {}. Error: {}", from_id, to.id(), incoming_amount, outgoing_amount, err))
        .await?;

    // this message is really important, if you want to recover the balance after a crash; all of
    // the "amount that need to be settled" must be summed and added to the account's "balance".
    debug!(
        "Account {} balance after fulfill: {}. Amount that needs to be settled: {}",
        to.id(),
        balance,
        amount_to_settle
    );

    if amount_to_settle == 0 {
        // so we might have some balance, but it's not over the threshold
        // this might still end up scheduling a no-op as we should really be comparing to
        // `settle_to` which we do not have access here.
        if balance > 0 {
            // so if we have the timeout configured, we should now make sure that there is a
            // timeout pending or new one is created right now.
            //
            // FIXME: when multiple nodes share a database and accounts, there is no coordination
            // between the nodes and there can be multiple settlements when only one is expected.
            // One way to avoid this would be to record a last_settled_at timestamp, making sure it
            // is always older than our settlement period, and rescheduling a timeout whenever it
            // would had been too early to settle.
            policy.settle_later(to.id(), channel_last_fail);
        }
        return Ok(());
    }

    // cancel a pending settlement always before trying it
    policy.clear_later(to.id(), channel_last_fail);

    settle_or_rollback(store, to, amount_to_settle, settlement_client).await
}

async fn settle_or_rollback<Store, Acct>(
    store: Store,
    to: Acct,
    amount: u64,
    client: SettlementClient,
) -> Result<(), ()>
where
    Store: SettlementStore<Account = Acct> + 'static,
    Acct: SettlementAccount + 'static,
{
    if amount == 0 {
        debug!("Nothing to settle for account {}", to.id());
        return Ok(());
    }

    if let Some(engine_details) = to.settlement_engine_details() {
        let engine_url = engine_details.url;
        // Note that if this program crashes after changing the balance (in the PROCESS_FULFILL
        // script) and the send_settlement fails but the program isn't alive to hear that, the
        // balance will be incorrect. No other instance will know that it was trying to send an
        // outgoing settlement. We could make this more robust by saving something to the DB about
        // the outgoing settlement when we change the balance but then we would also need to
        // prevent a situation where every connector instance is polling the settlement engine for
        // the status of each outgoing settlement and putting unnecessary load on the settlement
        // engine.

        let result = client
            .send_settlement(to.id(), engine_url, amount, to.asset_scale())
            .await;

        if let Err(client_error) = result {
            warn!(
                "Settlement for account {} for {} failed: {}",
                to.id(),
                amount,
                client_error
            );

            store
                .refund_settlement(to.id(), amount)
                .map_err(|e| {
                    error!(
                        "Refunding account {} after failed settlement failed, amount: {}: {}",
                        to.id(),
                        amount,
                        e
                    )
                })
                .await?;
        } else {
            info!(
                "Settlement for account {} for {} succeeded",
                to.id(),
                amount
            );
        }
    } else {
        debug!("Settlement for account {} for {} failed as the account has no settlement engine details",
            to.id(), amount);
    }

    Ok(())
}

/// Captures the behaviour of either operating in a delayed settlement or threshold-only
/// environment.
#[derive(Debug, Clone)]
enum Policy {
    ThresholdOnly,
    TimeBased(tokio::sync::mpsc::Sender<ManageTimeout>),
}

impl Policy {
    /// Called to clear a pending timeout, if there's any
    fn clear_later(&mut self, account_id: Uuid, channel_last_fail: Arc<Mutex<Instant>>) {
        match *self {
            Policy::ThresholdOnly => (),
            Policy::TimeBased(ref mut sender) => Policy::drop_error(
                sender.try_send(ManageTimeout::Clear(account_id)),
                channel_last_fail,
            ),
        }
    }

    /// Called to signal this account id needs to be settled later
    fn settle_later(&mut self, account_id: Uuid, channel_last_fail: Arc<Mutex<Instant>>) {
        match *self {
            Policy::ThresholdOnly => (),
            Policy::TimeBased(ref mut sender) => Policy::drop_error(
                sender.try_send(ManageTimeout::Set(account_id)),
                channel_last_fail,
            ),
        }
    }

    // Rationale for dropping the errors:
    //
    // If the channel is full of peer accounts, it is likely
    // that the peer account that the messages are directed towards
    // has already been scheduled for settlement.
    //
    // If the channel is full because the background task is
    // not keeping up, bounded channel prevents an infinitely-
    // growing queue from forming.
    //
    // If a later change introduces a bug in the background task,
    // resulting in it not processing the incoming messages,
    // bounded channel likewise prevents an infinitely-growing queue.
    //
    // If the messages are not being received because of the receiver
    // having closed, the background task has exited.
    // This normally happens on shutdown.
    //
    // Should sending through bounded channel fail (due to afore-
    // mentioned reasons), the intended time-based settlements are lost
    // as a result. The threshold-based settlement will, however,
    // eventually take over, acting as a "backup" in this situation.
    // Moreover the bounded channel failing should only happen
    // under an exceptionally heavy load.
    //
    // While the function drops the errors, it also logs them
    // once every 60 seconds. This is to prevent flooding the log
    // with (identical) error messages from -- most likely --
    // the very same problem.
    fn drop_error(
        result: Result<(), TrySendError<ManageTimeout>>,
        channel_last_fail: Arc<Mutex<Instant>>,
    ) {
        let (reason, uuid) = match result {
            Ok(_) => return,
            Err(TrySendError::Full(mto)) => ("full", mto.uuid()),
            Err(TrySendError::Closed(mto)) => ("closed", mto.uuid()),
        };

        // By using try_lock() we avoid worsening any overload situation by limiting unnecessary
        // logging action.
        if let Ok(ref mut t0) = channel_last_fail.try_lock() {
            let t = Instant::now();
            if t.duration_since(**t0).as_secs() >= 60 {
                warn!(
                    "Time-based settlement failed temporarily \
                    (bounded channel {}) for (at least) the account {}. \
                    Service might be overloaded. Check the balances \
                    once the overload has passed.",
                    reason, uuid
                );
                **t0 = t;
            }
        }
    }
}

/// When configured to operate with time based settlement `ManageTimeout` models the commands sent
/// over to background task to manage the timeouts.
pub enum ManageTimeout {
    Clear(Uuid),
    Set(Uuid),
}

impl ManageTimeout {
    fn uuid(&self) -> Uuid {
        match *self {
            ManageTimeout::Clear(uuid) => uuid,
            ManageTimeout::Set(uuid) => uuid,
        }
    }
}

#[derive(Debug)]
enum ExitReason {
    InputClosed,
    Shutdown,
    Capacity,
    Other(tokio::time::error::Error),
}

impl fmt::Display for ExitReason {
    fn fmt(&self, fmt: &mut fmt::Formatter) -> fmt::Result {
        use ExitReason::*;
        match *self {
            InputClosed => write!(fmt, "Input was closed and ran out of timed settlements"),
            Shutdown => write!(fmt, "Tokio is shutting down"),
            Capacity => write!(fmt, "Timer service capacity exceeded"),
            Other(ref e) => write!(fmt, "Other: {}", e),
        }
    }
}

/// Start a background task for time based settlement. If time based settlement is configured but
/// this task is never started, the time-based settlement does not happen and a warning is logged
/// every minute on eligble random peering account.
pub fn start_delayed_settlement<St, Store, Acct>(
    delay: Duration,
    cmds: St,
    store: Store,
) -> tokio::task::JoinHandle<()>
where
    St: futures::stream::FusedStream<Item = ManageTimeout> + Send + Sync + 'static + Unpin,
    Store: BalanceStore
        + SettlementStore<Account = Acct>
        + AccountStore<Account = Acct>
        + Clone
        + Send
        + Sync
        + 'static,
    Acct: SettlementAccount + Send + Sync + 'static,
{
    let client = SettlementClient::default();
    tokio::spawn(async move {
        info!(
            "Starting to run delayed settlements with a timeout of {:?}",
            delay
        );

        let exit_reason = run_timeouts_and_settle_on_delay(delay, cmds, store, client).await;

        info!(
            "Stopped running timeouts and delayed settlements: {}",
            exit_reason
        );
    })
}

async fn run_timeouts_and_settle_on_delay<St, Store, Acct>(
    delay: Duration,
    mut cmds: St,
    store: Store,
    client: SettlementClient,
) -> ExitReason
where
    St: futures::stream::FusedStream<Item = ManageTimeout> + Send + Sync + 'static + Unpin,
    Store: BalanceStore
        + SettlementStore<Account = Acct>
        + AccountStore<Account = Acct>
        + Clone
        + Send
        + Sync
        + 'static,
    Acct: SettlementAccount + Send + Sync + 'static,
{
    use futures::stream::StreamExt;
    use std::collections::HashMap;
    use tokio_util::time::DelayQueue;

    let mut timeouts = DelayQueue::new();
    let mut in_queue = HashMap::new();

    loop {
        tokio::select! {
            cmd = cmds.select_next_some() => {
                match cmd {
                    ManageTimeout::Clear(id) => {
                        let key = in_queue.remove(&id);

                        if let Some(key) = key {
                            // this should only be removed when the settle_threshold was achieved
                            // while processing a fulfill.
                            timeouts.remove(&key);
                            trace!("Cleared pending settlement timeout for account: {}", id);
                        }
                    }
                    ManageTimeout::Set(id) => {
                        let timeouts = &mut timeouts;
                        in_queue.entry(id).or_insert_with(move || {
                            let key = timeouts.insert(id, delay);

                            trace!("Setting pending settlement timeout for account: {}", id);

                            key
                        });
                    }
                }
            },
            next = timeouts.next(), if !timeouts.is_empty() || cmds.is_terminated() => {
                match next {
                    Some(Ok(expired)) => {
                        let id = expired.into_inner();
                        in_queue.remove(&id); // unsure if this can ever be none

                        trace!("Delayed settlement for account {} expired", id);

                        let client = client.clone();
                        let store = store.clone();

                        tokio::spawn(async move {
                            // bailing out instead of not re-scheduling on failing to load the
                            // account: it is assumed that if this account is valid and should be
                            // settled there is near-continouos traffic which would trigger either
                            // the threshold or time based settlement again.
                            let to = match store.get_accounts(vec![id]).await {
                                Ok(mut accounts) if accounts.len() == 1 => {
                                    Ok(accounts.pop().unwrap())
                                },
                                Ok(accounts) => {
                                    error!(
                                        "Asked for account {} for delayed settlement got back {} accounts: {:?}",
                                        id, accounts.len(), accounts
                                    );
                                    Err(())
                                }
                                Err(e) => {
                                    warn!(
                                        "Failed to load account {} for time-based settlement: {}",
                                        id, e
                                    );
                                    Err(())
                                }
                            }?;

                            let (balance, amount_to_settle) = store.update_balances_for_delayed_settlement(id).await
                                .map_err(|e| warn!("Time-based settlement failed for {}: {}", id, e))?;

                            debug!(
                                "Account {} balance at time-based settlement: {}, amount that needs to be settled: {}",
                                to.id(), balance, amount_to_settle
                            );

                            settle_or_rollback(store, to, amount_to_settle, client).await
                        });
                    },
                    Some(Err(e)) if e.is_shutdown() => {
                        // we probably cant do much better than to exit here (and to drop the
                        // stream)
                        return ExitReason::Shutdown;
                    },
                    Some(Err(e)) if e.is_at_capacity() => {
                        return ExitReason::Capacity;
                    },
                    Some(Err(e)) => {
                        return ExitReason::Other(e);
                    }
                    None => {
                        // no more timeouts currently
                        assert!(cmds.is_terminated());
                        // no more timeouts ever
                        return ExitReason::InputClosed;
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use interledger_errors::{AddressStoreError, SettlementStoreError};
    use interledger_packet::{Address, FulfillBuilder, PrepareBuilder, RejectBuilder};
    use interledger_settlement::core::types::SettlementEngineDetails;
    use once_cell::sync::Lazy;
    use parking_lot::RwLock;
    use std::str::FromStr;
    use std::sync::Arc;
    use std::time::Duration;
    use url::Url;

    #[tokio::test]
    async fn executes_settlement() {
        let mock = mockito::mock("POST", mockito::Matcher::Any).create();
        let next = outgoing_service_fn(move |_| {
            Ok(FulfillBuilder {
                fulfillment: &[0; 32],
                data: b"test data",
            }
            .build())
        });
        let store = TestStore::new(1);
        let mut service = BalanceService::new(store.clone(), None, next);
        let fulfill = service.send_request(TEST_REQUEST.clone()).await.unwrap();
        assert_eq!(fulfill.data(), b"test data");

        tokio::time::sleep(Duration::from_millis(100u64)).await;
        mock.assert();
        assert!(!*store.refunded_settlement.read());
        assert!(!*store.rejected_message.read());
    }

    #[tokio::test]
    async fn nothing_to_settle() {
        let mock = mockito::mock("POST", mockito::Matcher::Any)
            .create()
            .expect(0);
        let next = outgoing_service_fn(move |_| {
            Ok(FulfillBuilder {
                fulfillment: &[0; 32],
                data: b"test data",
            }
            .build())
        });
        let store = TestStore::new(0);
        let mut service = BalanceService::new(store.clone(), None, next);
        let fulfill = service.send_request(TEST_REQUEST.clone()).await.unwrap();
        assert_eq!(fulfill.data(), b"test data");

        tokio::time::sleep(Duration::from_millis(100u64)).await;
        mock.assert();
        assert!(!*store.refunded_settlement.read());
        assert!(!*store.rejected_message.read());
    }

    #[tokio::test]
    async fn executes_settlement_and_refunds() {
        let mock = mockito::mock("POST", mockito::Matcher::Any)
            .with_status(404)
            .create();
        let next = outgoing_service_fn(move |_| {
            Ok(FulfillBuilder {
                fulfillment: &[0; 32],
                data: b"test data",
            }
            .build())
        });
        let store = TestStore::new(1);
        let mut service = BalanceService::new(store.clone(), None, next);
        let fulfill = service.send_request(TEST_REQUEST.clone()).await.unwrap();
        assert_eq!(fulfill.data(), b"test data");

        tokio::time::sleep(Duration::from_millis(100u64)).await;
        mock.assert();
        assert!(*store.refunded_settlement.read());
        assert!(!*store.rejected_message.read());
    }

    #[tokio::test]
    async fn updates_for_reject() {
        let mock = mockito::mock("POST", mockito::Matcher::Any)
            .create()
            .expect(0);
        let next = outgoing_service_fn(move |_| {
            Err(RejectBuilder {
                code: ErrorCode::T00_INTERNAL_ERROR,
                message: &[],
                triggered_by: None,
                data: &[],
            }
            .build())
        });
        let store = TestStore::new(1);
        let mut service = BalanceService::new(store.clone(), None, next);
        let reject = service
            .send_request(TEST_REQUEST.clone())
            .await
            .unwrap_err();
        assert_eq!(reject.code(), ErrorCode::T00_INTERNAL_ERROR);

        tokio::time::sleep(Duration::from_millis(100u64)).await;
        mock.assert();
        assert!(*store.rejected_message.read());
    }

    #[derive(Debug, Clone)]
    struct TestAccount {
        pub engine_url: Url,
    }

    static ALICE: Lazy<Username> = Lazy::new(|| Username::from_str("alice").unwrap());
    static EXAMPLE_ADDRESS: Lazy<Address> =
        Lazy::new(|| Address::from_str("example.alice").unwrap());

    impl Account for TestAccount {
        fn id(&self) -> Uuid {
            Uuid::new_v4()
        }

        fn username(&self) -> &Username {
            &ALICE
        }

        fn asset_code(&self) -> &str {
            "XYZ"
        }

        // All connector accounts use asset scale = 9.
        fn asset_scale(&self) -> u8 {
            9
        }

        fn ilp_address(&self) -> &Address {
            &EXAMPLE_ADDRESS
        }
    }

    impl SettlementAccount for TestAccount {
        fn settlement_engine_details(&self) -> Option<SettlementEngineDetails> {
            Some(SettlementEngineDetails {
                url: self.engine_url.clone(),
            })
        }
    }

    #[derive(Clone)]
    struct TestStore {
        amount_to_settle: u64,
        rejected_message: Arc<RwLock<bool>>,
        refunded_settlement: Arc<RwLock<bool>>,
    }

    impl TestStore {
        fn new(amount_to_settle: u64) -> Self {
            TestStore {
                amount_to_settle,
                rejected_message: Arc::new(RwLock::new(false)),
                refunded_settlement: Arc::new(RwLock::new(false)),
            }
        }
    }

    #[async_trait]
    impl AddressStore for TestStore {
        async fn set_ilp_address(&self, _: Address) -> Result<(), AddressStoreError> {
            unimplemented!()
        }

        async fn clear_ilp_address(&self) -> Result<(), AddressStoreError> {
            unimplemented!()
        }

        fn get_ilp_address(&self) -> Address {
            Address::from_str("example.connector").unwrap()
        }
    }

    #[async_trait]
    impl BalanceStore for TestStore {
        async fn get_balance(&self, _: Uuid) -> Result<i64, BalanceStoreError> {
            unimplemented!()
        }

        async fn update_balances_for_prepare(
            &self,
            _: Uuid,
            _: u64,
        ) -> Result<(), BalanceStoreError> {
            Ok(())
        }

        async fn update_balances_for_fulfill(
            &self,
            _: Uuid,
            _: u64,
        ) -> Result<(i64, u64), BalanceStoreError> {
            Ok((0, self.amount_to_settle))
        }

        async fn update_balances_for_reject(
            &self,
            _: Uuid,
            _: u64,
        ) -> Result<(), BalanceStoreError> {
            *self.rejected_message.write() = true;
            Ok(())
        }

        async fn update_balances_for_delayed_settlement(
            &self,
            _: Uuid,
        ) -> Result<(i64, u64), BalanceStoreError> {
            Ok((0, self.amount_to_settle))
        }
    }

    #[async_trait]
    impl SettlementStore for TestStore {
        type Account = TestAccount;

        async fn update_balance_for_incoming_settlement(
            &self,
            _: Uuid,
            _: u64,
            _: Option<String>,
        ) -> Result<(), SettlementStoreError> {
            Ok(())
        }

        async fn refund_settlement(&self, _: Uuid, _: u64) -> Result<(), SettlementStoreError> {
            *self.refunded_settlement.write() = true;
            Ok(())
        }
    }

    static TEST_REQUEST: Lazy<OutgoingRequest<TestAccount>> = Lazy::new(|| {
        let url = mockito::server_url();
        OutgoingRequest {
            to: TestAccount {
                engine_url: Url::parse(&url).unwrap(),
            },
            from: TestAccount {
                engine_url: Url::parse(&url).unwrap(),
            },
            original_amount: 100,
            prepare: PrepareBuilder {
                destination: Address::from_str("example.destination").unwrap(),
                amount: 100,
                expires_at: std::time::SystemTime::now() + std::time::Duration::from_secs(30),
                execution_condition: &[0; 32],
                data: b"test data",
            }
            .build(),
        }
    });
}
