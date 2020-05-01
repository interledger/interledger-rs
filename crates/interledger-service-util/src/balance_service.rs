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
use tracing::{debug, error};
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
    account_type: PhantomData<A>,
}

impl<S, O, A> BalanceService<S, O, A>
where
    S: AddressStore + BalanceStore + SettlementStore<Account = A>,
    O: OutgoingService<A>,
    A: Account + SettlementAccount,
{
    pub fn new(store: S, next: O) -> Self {
        BalanceService {
            store,
            next,
            settlement_client: SettlementClient::default(),
            account_type: PhantomData,
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
            return self.next.send_request(request).await;
        }

        let mut next = self.next.clone();
        let store = self.store.clone();
        let from = request.from.clone();
        let from_clone = from.clone();
        let from_id = from.id();
        let to = request.to.clone();
        let to_clone = to.clone();
        let to_id = to.id();
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
            .update_balances_for_prepare(from.id(), incoming_amount)
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
                    tokio::spawn(async move {
                        let (balance, amount_to_settle) = store
                            .update_balances_for_fulfill(to.id(), outgoing_amount)
                            .map_err(|err| error!("Error applying balance changes for fulfill from account: {} to account: {}. Incoming amount was: {}, outgoing amount was: {}. Error: {}", from_id, to_id, incoming_amount, outgoing_amount, err))
                            .await?;
                        debug!(
                            "Account balance after fulfill: {}. Amount that needs to be settled: {}",
                            balance, amount_to_settle
                        );
                        if amount_to_settle > 0 {
                            if let Some(engine_details) = to.settlement_engine_details() {
                                let engine_url = engine_details.url;
                                // Note that if this program crashes after changing the balance (in the PROCESS_FULFILL script)
                                // and the send_settlement fails but the program isn't alive to hear that, the balance will be incorrect.
                                // No other instance will know that it was trying to send an outgoing settlement. We could
                                // make this more robust by saving something to the DB about the outgoing settlement when we change the balance
                                // but then we would also need to prevent a situation where every connector instance is polling the
                                // settlement engine for the status of each
                                // outgoing settlement and putting unnecessary
                                // load on the settlement engine.
                                if settlement_client
                                    .send_settlement(
                                        to.id(),
                                        engine_url,
                                        amount_to_settle,
                                        to.asset_scale(),
                                    )
                                    .await
                                    .is_err()
                                {
                                    store
                                        .refund_settlement(to_id, amount_to_settle)
                                        .map_err(|_| ())
                                        .await?;
                                }
                            }
                        }
                        Ok::<(), ()>(())
                    });
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
        let mut service = BalanceService::new(store.clone(), next);
        let fulfill = service.send_request(TEST_REQUEST.clone()).await.unwrap();
        assert_eq!(fulfill.data(), b"test data");

        tokio::time::delay_for(Duration::from_millis(100u64)).await;
        mock.assert();
        assert_eq!(*store.refunded_settlement.read(), false);
        assert_eq!(*store.rejected_message.read(), false);
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
        let mut service = BalanceService::new(store.clone(), next);
        let fulfill = service.send_request(TEST_REQUEST.clone()).await.unwrap();
        assert_eq!(fulfill.data(), b"test data");

        tokio::time::delay_for(Duration::from_millis(100u64)).await;
        mock.assert();
        assert_eq!(*store.refunded_settlement.read(), false);
        assert_eq!(*store.rejected_message.read(), false);
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
        let mut service = BalanceService::new(store.clone(), next);
        let fulfill = service.send_request(TEST_REQUEST.clone()).await.unwrap();
        assert_eq!(fulfill.data(), b"test data");

        tokio::time::delay_for(Duration::from_millis(100u64)).await;
        mock.assert();
        assert_eq!(*store.refunded_settlement.read(), true);
        assert_eq!(*store.rejected_message.read(), false);
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
        let mut service = BalanceService::new(store.clone(), next);
        let reject = service
            .send_request(TEST_REQUEST.clone())
            .await
            .unwrap_err();
        assert_eq!(reject.code(), ErrorCode::T00_INTERNAL_ERROR);

        tokio::time::delay_for(Duration::from_millis(100u64)).await;
        mock.assert();
        assert_eq!(*store.rejected_message.read(), true);
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
