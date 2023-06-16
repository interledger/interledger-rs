use async_trait::async_trait;
use interledger_packet::{ErrorCode, RejectBuilder};
use interledger_rates::ExchangeRateStore;
use interledger_service::*;
use interledger_settlement::core::types::{ConversionError, Convert, ConvertDetails};
use std::marker::PhantomData;
use tracing::{error, trace, warn};

/// # Exchange Rates Service
///
/// Responsible for getting the exchange rates for the two assets in the outgoing request (`request.from.asset_code`, `request.to.asset_code`).
/// Requires a `ExchangeRateStore`
#[derive(Clone)]
pub struct ExchangeRateService<S, O, A> {
    spread: f64,
    store: S,
    next: O,
    account_type: PhantomData<A>,
}

impl<S, O, A> ExchangeRateService<S, O, A>
where
    S: AddressStore + ExchangeRateStore,
    O: OutgoingService<A>,
    A: Account,
{
    pub fn new(spread: f64, store: S, next: O) -> Self {
        ExchangeRateService {
            spread,
            store,
            next,
            account_type: PhantomData,
        }
    }
}

#[async_trait]
impl<S, O, A> OutgoingService<A> for ExchangeRateService<S, O, A>
where
    // TODO can we make these non-'static?
    S: AddressStore + ExchangeRateStore + Clone + Send + Sync + 'static,
    O: OutgoingService<A> + Send + Sync + Clone + 'static,
    A: Account + Send + Sync + 'static,
{
    /// On send request:
    /// 1. If the prepare packet's amount is 0, it just forwards
    /// 1. Retrieves the exchange rate from the store (the store independently is responsible for polling the rates)
    ///     - return reject if the call to the store fails
    /// 1. Calculates the exchange rate AND scales it up/down depending on how many decimals each asset requires
    /// 1. Updates the amount in the prepare packet and forwards it
    async fn send_request(&mut self, mut request: OutgoingRequest<A>) -> IlpResult {
        let ilp_address = self.store.get_ilp_address();
        if request.prepare.amount() > 0 {
            let rates: (f64, f64) = if request.from.asset_code() == request.to.asset_code() {
                (1f64, 1f64)
            } else if let Ok(rates) = self
                .store
                .get_exchange_rates(&[request.from.asset_code(), request.to.asset_code()])
            {
                // Exchange rates are expressed as `base asset / asset`. To calculate the outgoing amount,
                // we multiply by the incoming asset's rate and divide by the outgoing asset's rate. For example,
                // if an incoming packet is denominated in an asset worth 1 USD and the outgoing asset is worth
                // 10 USD, the outgoing amount will be 1/10th of the source amount.
                (rates[0], rates[1])
            } else {
                error!(
                    "No exchange rates available for assets: {}, {}",
                    request.from.asset_code(),
                    request.to.asset_code()
                );
                return Err(RejectBuilder {
                    code: ErrorCode::T00_INTERNAL_ERROR,
                    message: format!(
                        "No exchange rate available from asset: {} to: {}",
                        request.from.asset_code(),
                        request.to.asset_code()
                    )
                    .as_bytes(),
                    triggered_by: Some(&ilp_address),
                    data: &[],
                }
                .build());
            };

            // Can we overflow here?
            let outgoing_amount = calculate_outgoing_amount(
                request.prepare.amount(),
                self.spread,
                rates,
                (request.from.asset_scale(), request.to.asset_scale()),
            );

            match outgoing_amount {
                Ok(outgoing_amount) => {
                    request.prepare.set_amount(outgoing_amount);
                    trace!("Converted incoming amount of: {} {} (scale {}) from account {} to outgoing amount of: {} {} (scale {}) for account {}",
                        request.original_amount, request.from.asset_code(), request.from.asset_scale(), request.from.id(),
                        outgoing_amount, request.to.asset_code(), request.to.asset_scale(), request.to.id());
                }
                Err(outgoing_amount_error) => {
                    let (code, message) = match outgoing_amount_error {
                        // Amount was too small to be converted to a non-zero u64, i.e. smaller
                        // than 1.0.
                        OutgoingAmountError::LessThanOne(outgoing_amount) => (
                            ErrorCode::R01_INSUFFICIENT_SOURCE_AMOUNT,
                            format!(
                                "Could not cast to f64, amount too small: {}",
                                outgoing_amount
                            ),
                        ),
                        // Amount was too large to be converted to u64 from f64, i.e. greater
                        // than u64::MAX as f64.
                        OutgoingAmountError::ToU64ConvertOverflow(outgoing_amount) => (
                            ErrorCode::F08_AMOUNT_TOO_LARGE,
                            format!(
                                "Could not cast to f64, amount too large: {}",
                                outgoing_amount
                            ),
                        ),
                        OutgoingAmountError::FloatOverflow => (
                            ErrorCode::F08_AMOUNT_TOO_LARGE,
                            format!(
                                "Could not convert exchange rate from {}:{} to: {}:{}. Got incoming amount: {}",
                                request.from.asset_code(),
                                request.from.asset_scale(),
                                request.to.asset_code(),
                                request.to.asset_scale(),
                                request.prepare.amount(),
                            ),
                        ),
                    };
                    return Err(RejectBuilder {
                        code,
                        message: message.as_bytes(),
                        triggered_by: Some(&ilp_address),
                        data: &[],
                    }
                    .build());
                }
            };
        }

        self.next.send_request(request).await
    }
}

#[derive(PartialEq, Debug)]
enum OutgoingAmountError {
    ToU64ConvertOverflow(f64),
    FloatOverflow,
    LessThanOne(f64),
}

fn calculate_outgoing_amount(
    input: u64,
    spread: f64,
    (rate_src, rate_dest): (f64, f64),
    (asset_scale_src, asset_scale_dest): (u8, u8),
) -> Result<u64, OutgoingAmountError> {
    let rate = rate_src / rate_dest;
    // Apply spread
    // TODO should this be applied differently for "local" or same-currency packets?
    let rate = rate * (1.0 - spread);
    let rate = if rate.is_finite() && rate.is_sign_positive() {
        rate
    } else {
        warn!(
            "Exchange rate would have been {} based on rate and spread, using 0.0 instead",
            rate
        );
        0.0
    };
    // Can we overflow here?
    let outgoing_amount = 1.0f64
        .normalize_scale(ConvertDetails {
            from: asset_scale_src,
            to: asset_scale_dest,
        })
        .map(|scale| rate * scale * (input as f64));

    match outgoing_amount {
        // Happens when rate == 0 or spread >= 1
        // In latter case the node takes everything to itself
        Ok(x) if x == 0.0f64 => Ok(0),
        Ok(x) if x < 1.0f64 => Err(OutgoingAmountError::LessThanOne(x)),
        Ok(x) if !x.is_finite() => Err(OutgoingAmountError::FloatOverflow),
        // FIXME: u64::MAX is higher than 2^53 or whatever is the max integer precision in f64
        Ok(x) if x > u64::MAX as f64 => Err(OutgoingAmountError::ToU64ConvertOverflow(x)),
        Ok(x) => Ok(x as u64),
        // Error happens if float happens to be std::f64::INFINITY after conversion
        Err(ConversionError) => Err(OutgoingAmountError::FloatOverflow),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use interledger_errors::{AddressStoreError, ExchangeRateStoreError};
    use interledger_packet::{Address, Fulfill, FulfillBuilder, PrepareBuilder, Reject};
    use interledger_service::{outgoing_service_fn, Account};
    use once_cell::sync::Lazy;
    use std::collections::HashMap;
    use std::str::FromStr;
    use std::{
        sync::{Arc, Mutex},
        time::SystemTime,
    };
    use uuid::Uuid;

    pub static ALICE: Lazy<Username> = Lazy::new(|| Username::from_str("alice").unwrap());

    #[tokio::test]
    async fn exchange_rate_ok() {
        // if `to` is worth $2, and `from` is worth 1, then they receive half
        // the amount of units
        let ret = exchange_rate(200, 1, 1.0, 1, 2.0, 0.0).await;
        assert_eq!(ret.1[0].prepare.amount(), 100);

        let ret = exchange_rate(1_000_000, 1, 3.0, 1, 2.0, 0.0).await;
        assert_eq!(ret.1[0].prepare.amount(), 1_500_000);
    }

    #[tokio::test]
    async fn exchange_conversion_error() {
        // rejects f64 that does not fit in u64
        let ret = exchange_rate(std::u64::MAX, 1, 2.0, 1, 1.0, 0.0).await;
        let reject = ret.0.unwrap_err();
        assert_eq!(reject.code(), ErrorCode::F08_AMOUNT_TOO_LARGE);
        assert!(reject
            .message()
            .starts_with(b"Could not cast to f64, amount too large"));

        // rejects f64 which gets rounded down to 0
        let ret = exchange_rate(1, 2, 1.0, 1, 1.0, 0.0).await;
        let reject = ret.0.unwrap_err();
        assert_eq!(reject.code(), ErrorCode::R01_INSUFFICIENT_SOURCE_AMOUNT);
        assert!(reject
            .message()
            .starts_with(b"Could not cast to f64, amount too small"));

        // `Convert` errored
        let ret = exchange_rate(std::u64::MAX, 1, std::f64::MAX, 255, 1.0, 0.0).await;
        let reject = ret.0.unwrap_err();
        assert_eq!(reject.code(), ErrorCode::F08_AMOUNT_TOO_LARGE);
        assert!(reject.message().starts_with(b"Could not convert"));
    }

    #[tokio::test]
    async fn applies_spread() {
        let ret = exchange_rate(100, 1, 1.0, 1, 2.0, 0.01).await;
        assert_eq!(ret.1[0].prepare.amount(), 49);

        // Negative spread is unusual but possible
        let ret = exchange_rate(200, 1, 1.0, 1, 2.0, -0.01).await;
        assert_eq!(ret.1[0].prepare.amount(), 101);

        // Rounds down
        let ret = exchange_rate(4, 1, 1.0, 1, 2.0, 0.01).await;
        // this would've been 2, but it becomes 1.99 and gets rounded down to 1
        assert_eq!(ret.1[0].prepare.amount(), 1);

        // Spread >= 1 means the node takes everything
        let ret = exchange_rate(10_000_000_000, 1, 1.0, 1, 2.0, 1.0).await;
        assert_eq!(ret.1[0].prepare.amount(), 0);

        // Need to catch when spread > 1
        let ret = exchange_rate(10_000_000_000, 1, 1.0, 1, 2.0, 2.0).await;
        assert_eq!(ret.1[0].prepare.amount(), 0);
    }

    // Errors most likely are caused by floating point errors
    #[test]
    fn calculates_with_small_input() {
        for i in 1..100 {
            assert_eq!(
                calculate_outgoing_amount(i, 0.0, (0.00000025, 0.25), (0, 6)),
                Ok(i)
            );
        }
    }
    // Errors most likely are caused by floating point errors
    #[test]
    fn calculates_with_big_input() {
        assert_eq!(
            calculate_outgoing_amount(159000000000, 0.0, (0.000009, 1.0), (3, 0)),
            Ok(1431)
        );
    }

    #[test]
    fn calculates_with_positive_spread() {
        assert_eq!(
            calculate_outgoing_amount(50, 0.11, (1.0, 1.0), (0, 0)),
            Ok(44)
        );
    }

    #[test]
    fn calculates_with_maximum_spread() {
        assert_eq!(
            calculate_outgoing_amount(50, 1.0, (1.0, 1.0), (0, 0)),
            Ok(0)
        );
    }

    #[test]
    fn calculates_with_negative_spread() {
        assert_eq!(
            calculate_outgoing_amount(50, -0.11, (1.0, 1.0), (0, 0)),
            Ok(55)
        );
    }

    #[test]
    fn calculates_with_u64_convert_overflow() {
        assert_eq!(
            calculate_outgoing_amount(u64::MAX, 0.0, (1.0, 1.0), (0, 1)),
            Err(OutgoingAmountError::ToU64ConvertOverflow(
                184467440737095500000.0
            ))
        );
    }

    #[test]
    fn calculates_with_float_overflow() {
        assert_eq!(
            calculate_outgoing_amount(u64::MAX, 0.0, (f64::MAX, 1.0), (0, 255)),
            Err(OutgoingAmountError::FloatOverflow)
        );
    }

    #[test]
    fn calculates_with_less_than_one() {
        assert_eq!(
            calculate_outgoing_amount(1, 0.0, (1.0, 2.0), (0, 0)),
            Err(OutgoingAmountError::LessThanOne(0.5))
        );
    }

    #[test]
    fn calculates_with_high_asset_scale() {
        assert_eq!(
            calculate_outgoing_amount(10, 0.0, (1.0, 1.0), (i8::MAX as u8 + 1, i8::MAX as u8)),
            Ok(1)
        );
    }

    // Instantiates an exchange rate service and returns the fulfill/reject
    // packet and the outgoing request after performing an asset conversion
    async fn exchange_rate(
        amount: u64,
        scale1: u8,
        rate1: f64,
        scale2: u8,
        rate2: f64,
        spread: f64,
    ) -> (Result<Fulfill, Reject>, Vec<OutgoingRequest<TestAccount>>) {
        let requests = Arc::new(Mutex::new(Vec::new()));
        let requests_clone = requests.clone();
        let outgoing = outgoing_service_fn(move |request| {
            requests_clone.lock().unwrap().push(request);
            Ok(FulfillBuilder {
                fulfillment: &[0; 32],
                data: b"hello!",
            }
            .build())
        });
        let mut service = test_service(rate1, rate2, spread, outgoing);
        let result = service
            .send_request(OutgoingRequest {
                from: TestAccount::new("ABC".to_owned(), scale1),
                to: TestAccount::new("XYZ".to_owned(), scale2),
                original_amount: amount,
                prepare: PrepareBuilder {
                    destination: Address::from_str("example.destination").unwrap(),
                    amount,
                    expires_at: SystemTime::now(),
                    execution_condition: &[1; 32],
                    data: b"hello",
                }
                .build(),
            })
            .await;

        let reqs = requests.lock().unwrap();
        (result, reqs.clone())
    }

    #[derive(Debug, Clone)]
    struct TestAccount {
        ilp_address: Address,
        asset_code: String,
        asset_scale: u8,
    }
    impl TestAccount {
        fn new(asset_code: String, asset_scale: u8) -> Self {
            TestAccount {
                ilp_address: Address::from_str("example.alice").unwrap(),
                asset_code,
                asset_scale,
            }
        }
    }

    #[async_trait]
    impl AddressStore for TestStore {
        /// Saves the ILP Address in the store's memory and database
        async fn set_ilp_address(&self, _ilp_address: Address) -> Result<(), AddressStoreError> {
            unimplemented!()
        }

        async fn clear_ilp_address(&self) -> Result<(), AddressStoreError> {
            unimplemented!()
        }

        /// Get's the store's ilp address from memory
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
            &self.asset_code
        }

        fn asset_scale(&self) -> u8 {
            self.asset_scale
        }

        fn ilp_address(&self) -> &Address {
            &self.ilp_address
        }
    }

    #[derive(Debug, Clone)]
    struct TestStore {
        rates: HashMap<Vec<String>, (f64, f64)>,
    }

    impl ExchangeRateStore for TestStore {
        fn get_exchange_rates(
            &self,
            asset_codes: &[&str],
        ) -> Result<Vec<f64>, ExchangeRateStoreError> {
            let mut ret = Vec::new();
            let key = vec![asset_codes[0].to_owned(), asset_codes[1].to_owned()];
            let v = self.rates.get(&key);
            if let Some(v) = v {
                ret.push(v.0);
                ret.push(v.1);
            } else {
                return Err(ExchangeRateStoreError::PairNotFound {
                    from: key[0].clone(),
                    to: key[1].clone(),
                });
            }
            Ok(ret)
        }

        fn set_exchange_rates(
            &self,
            _rates: HashMap<String, f64>,
        ) -> Result<(), ExchangeRateStoreError> {
            unimplemented!()
        }

        fn get_all_exchange_rates(&self) -> Result<HashMap<String, f64>, ExchangeRateStoreError> {
            unimplemented!()
        }
    }

    fn test_store(rate1: f64, rate2: f64) -> TestStore {
        let mut rates = HashMap::new();
        rates.insert(vec!["ABC".to_owned(), "XYZ".to_owned()], (rate1, rate2));
        TestStore { rates }
    }

    fn test_service(
        rate1: f64,
        rate2: f64,
        spread: f64,
        handler: impl OutgoingService<TestAccount> + Clone + Send + Sync,
    ) -> ExchangeRateService<
        TestStore,
        impl OutgoingService<TestAccount> + Clone + Send + Sync,
        TestAccount,
    > {
        let store = test_store(rate1, rate2);
        ExchangeRateService::new(spread, store, handler)
    }
}
