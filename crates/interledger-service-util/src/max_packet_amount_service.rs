use async_trait::async_trait;
use interledger_packet::{ErrorCode, MaxPacketAmountDetails, RejectBuilder};
use interledger_service::*;
use log::debug;

/// Extension trait for [`Account`](../interledger_service/trait.Account.html) with the max packet amount
/// allowed for this account
pub trait MaxPacketAmountAccount: Account {
    fn max_packet_amount(&self) -> u64;
}

/// # MaxPacketAmount Service
///
/// This service is used by nodes to limit the maximum value of each packet they are willing to forward.
/// Nodes may limit the packet amount for a variety of reasons:
/// - Liquidity: a node operator may not way to allow a single high-value packet to tie up a large portion of its liquidity at once (especially because they do not know whether the packet will be fulfilled or rejected)
/// - Security: each packet carries some risk, due to the possibility that a node's failure to pass back the fulfillment within the available time window would cause that node to lose money. Keeping the value of each individual packet low may help reduce the impact of such a failure
/// Signaling: nodes SHOULD set the maximum packet amount _lower_ than the maximum amount in flight (also known as the payment or money bandwidth). `T04: Insufficient Liquidity` errors do not communicate to the sender how much they can send, largely because the "available liquidity" may be time based or based on the rate of other payments going through and thus difficult to communicate effectively. In contrast, the `F08: Amount Too Large` error conveys the maximum back to the sender, because this limit is assumed to be a static value, and alllows sender-side software like STREAM implementations to respond accordingly. Therefore, setting the maximum packet amount lower than the total money bandwidth allows client implementations to quickly adjust their packet amounts to appropriate levels.
/// Requires a `MaxPacketAmountAccount` and _no store_.
#[derive(Clone)]
pub struct MaxPacketAmountService<I, S> {
    next: I,
    store: S,
}

impl<I, S> MaxPacketAmountService<I, S> {
    /// Simple constructor
    pub fn new(store: S, next: I) -> Self {
        MaxPacketAmountService { store, next }
    }
}

#[async_trait]
impl<I, S, A> IncomingService<A> for MaxPacketAmountService<I, S>
where
    I: IncomingService<A> + Send + Sync + 'static,
    S: AddressStore + Send + Sync + 'static,
    A: MaxPacketAmountAccount + Send + Sync + 'static,
{
    /// On receive request:
    /// 1. if request.prepare.amount <= request.from.max_packet_amount forward the request, else error
    async fn handle_request(&mut self, request: IncomingRequest<A>) -> IlpResult {
        let ilp_address = self.store.get_ilp_address();
        let max_packet_amount = request.from.max_packet_amount();
        if request.prepare.amount() <= max_packet_amount {
            self.next.handle_request(request).await
        } else {
            debug!(
                "Prepare amount:{} exceeds max_packet_amount: {}",
                request.prepare.amount(),
                max_packet_amount
            );
            let details =
                MaxPacketAmountDetails::new(request.prepare.amount(), max_packet_amount).to_bytes();
            Err(RejectBuilder {
                code: ErrorCode::F08_AMOUNT_TOO_LARGE,
                message: &[],
                triggered_by: Some(&ilp_address),
                data: &details[..],
            }
            .build())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use interledger_errors::AddressStoreError;
    use interledger_packet::{Address, FulfillBuilder, PrepareBuilder};
    use once_cell::sync::Lazy;
    use std::str::FromStr;
    use uuid::Uuid;

    #[derive(Debug, Clone)]
    struct TestAccount(u64);

    impl MaxPacketAmountAccount for TestAccount {
        fn max_packet_amount(&self) -> u64 {
            self.0
        }
    }

    #[tokio::test]
    async fn below_max_amount() {
        let next = incoming_service_fn(move |_| {
            Ok(FulfillBuilder {
                fulfillment: &[0; 32],
                data: b"test data",
            }
            .build())
        });
        let store = TestStore;

        let request = IncomingRequest {
            from: TestAccount(101),
            prepare: PrepareBuilder {
                destination: Address::from_str("example.destination").unwrap(),
                amount: 100,
                expires_at: std::time::SystemTime::now() + std::time::Duration::from_secs(30),
                execution_condition: &[0; 32],
                data: b"test data",
            }
            .build(),
        };

        let mut service = MaxPacketAmountService::new(store.clone(), next);
        let fulfill = service.handle_request(request).await.unwrap();
        assert_eq!(fulfill.data(), b"test data");
    }

    #[tokio::test]
    async fn above_max_amount() {
        let next = incoming_service_fn(move |_| {
            Ok(FulfillBuilder {
                fulfillment: &[0; 32],
                data: b"test data",
            }
            .build())
        });
        let store = TestStore;

        let request = IncomingRequest {
            from: TestAccount(99),
            prepare: PrepareBuilder {
                destination: Address::from_str("example.destination").unwrap(),
                amount: 100,
                expires_at: std::time::SystemTime::now() + std::time::Duration::from_secs(30),
                execution_condition: &[0; 32],
                data: b"test data",
            }
            .build(),
        };

        let mut service = MaxPacketAmountService::new(store.clone(), next);
        let reject = service.handle_request(request).await.unwrap_err();
        assert_eq!(reject.code(), ErrorCode::F08_AMOUNT_TOO_LARGE);
    }

    #[derive(Clone)]
    struct TestStore;

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

    static ALICE: Lazy<Username> = Lazy::new(|| Username::from_str("alice").unwrap());
    static EXAMPLE_ADDRESS: Lazy<Address> =
        Lazy::new(|| Address::from_str("example.alice").unwrap());
}
