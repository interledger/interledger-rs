use super::exchange_rate_providers::*;
use async_trait::async_trait;
use futures::TryFutureExt;
use interledger_errors::ExchangeRateStoreError;
use interledger_packet::{ErrorCode, RejectBuilder};
use interledger_service::*;
use interledger_settlement::core::types::{Convert, ConvertDetails};
use log::{debug, error, trace, warn};
use reqwest::Client;
use secrecy::SecretString;
use serde::Deserialize;
use std::{
    collections::HashMap,
    marker::PhantomData,
    sync::{
        atomic::{AtomicU32, Ordering},
        Arc,
    },
    time::Duration,
};

// TODO should this whole file be moved to its own crate?

/// Store trait responsible for managing the exchange rates of multiple currencies
pub trait ExchangeRateStore: Clone {
    // TODO we may want to make this async if/when we use pubsub to broadcast
    // rate changes to different instances of a horizontally-scalable node
    /// Sets the exchange rate by providing an AssetCode->USD price mapping
    fn set_exchange_rates(&self, rates: HashMap<String, f64>)
        -> Result<(), ExchangeRateStoreError>;

    /// Gets the exchange rates for the provided asset codes
    fn get_exchange_rates(&self, asset_codes: &[&str]) -> Result<Vec<f64>, ExchangeRateStoreError>;

    // TODO should this be on the API instead? That's where it's actually used
    // TODO should we combine this method with get_exchange_rates?
    // The downside of doing that is in this case we want a HashMap with owned values
    // (so that we don't accidentally lock up the RwLock on the store's exchange_rates)
    // but in the normal case of getting the rate between two assets, we don't want to
    // copy all the rate data
    /// Gets the exchange rates for all stored asset codes
    fn get_all_exchange_rates(&self) -> Result<HashMap<String, f64>, ExchangeRateStoreError>;
}

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
            let rate: f64 = if request.from.asset_code() == request.to.asset_code() {
                1f64
            } else if let Ok(rates) = self
                .store
                .get_exchange_rates(&[&request.from.asset_code(), &request.to.asset_code()])
            {
                // Exchange rates are expressed as `base asset / asset`. To calculate the outgoing amount,
                // we multiply by the incoming asset's rate and divide by the outgoing asset's rate. For example,
                // if an incoming packet is denominated in an asset worth 1 USD and the outgoing asset is worth
                // 10 USD, the outgoing amount will be 1/10th of the source amount.
                rates[0] / rates[1]
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

            // Apply spread
            // TODO should this be applied differently for "local" or same-currency packets?
            let rate = rate * (1.0 - self.spread);
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
            let outgoing_amount = (request.prepare.amount() as f64) * rate;
            let outgoing_amount = outgoing_amount.normalize_scale(ConvertDetails {
                from: request.from.asset_scale(),
                to: request.to.asset_scale(),
            });

            match outgoing_amount {
                Ok(outgoing_amount) => {
                    // The conversion succeeded, but the produced f64
                    // is larger than the maximum value for a u64.
                    // When it gets cast to a u64, it will end up being 0.
                    if outgoing_amount != 0.0 && outgoing_amount as u64 == 0 {
                        let (code, message) = if outgoing_amount < 1.0 {
                            // user wanted to send a positive value but it got rounded down to 0
                            (
                                ErrorCode::R01_INSUFFICIENT_SOURCE_AMOUNT,
                                format!(
                                    "Could not cast to f64, amount too small: {}",
                                    outgoing_amount
                                ),
                            )
                        } else {
                            // amount that arrived was too large for us to forward
                            (
                                ErrorCode::F08_AMOUNT_TOO_LARGE,
                                format!(
                                    "Could not cast to f64, amount too large: {}",
                                    outgoing_amount
                                ),
                            )
                        };

                        return Err(RejectBuilder {
                            code,
                            message: message.as_bytes(),
                            triggered_by: Some(&ilp_address),
                            data: &[],
                        }
                        .build());
                    }
                    request.prepare.set_amount(outgoing_amount as u64);
                    trace!("Converted incoming amount of: {} {} (scale {}) from account {} to outgoing amount of: {} {} (scale {}) for account {}",
                        request.original_amount, request.from.asset_code(), request.from.asset_scale(), request.from.id(),
                        outgoing_amount, request.to.asset_code(), request.to.asset_scale(), request.to.id());
                }
                Err(_) => {
                    // This branch gets executed when the `Convert` trait
                    // returns an error. Happens due to float
                    // multiplication overflow .
                    // (float overflow in Rust produces +inf)
                    return Err(RejectBuilder {
                        code: ErrorCode::F08_AMOUNT_TOO_LARGE,
                        message: format!(
                            "Could not convert exchange rate from {}:{} to: {}:{}. Got incoming amount: {}",
                            request.from.asset_code(),
                            request.from.asset_scale(),
                            request.to.asset_code(),
                            request.to.asset_scale(),
                            request.prepare.amount(),
                        )
                        .as_bytes(),
                        triggered_by: Some(&ilp_address),
                        data: &[],
                    }
                    .build());
                }
            }
        }

        self.next.send_request(request).await
    }
}

/// This determines which external API service to poll for exchange rates.
#[derive(Debug, Clone, Deserialize)]
pub enum ExchangeRateProvider {
    /// Use the [CoinCap] API.
    ///
    /// Note that when configured with YAML, this MUST be specified as
    /// "CoinCap", not "coincap".
    ///
    /// [CoinCap]: https://coincap.io/
    #[serde(alias = "coincap")]
    CoinCap,
    /// Use the [CryptoCompare] API. Note this service requires an
    /// API key (but the free tier supports 100,000 requests / month at the
    /// time of writing).
    ///
    /// Note that when configured with YAML, this MUST be specified as
    /// "CryptoCompare", not "crypto_compare".
    ///
    /// [CryptoCompare]: https://cryptocompare.com
    #[serde(alias = "cryptocompare")]
    CryptoCompare(SecretString),
}

/// Poll exchange rate providers for the current exchange rates
#[derive(Clone)]
pub struct ExchangeRateFetcher<S> {
    provider: ExchangeRateProvider,
    consecutive_failed_polls: Arc<AtomicU32>,
    failed_polls_before_invalidation: u32,
    store: S,
    client: Client,
}

impl<S> ExchangeRateFetcher<S>
where
    S: ExchangeRateStore + Send + Sync + 'static,
{
    /// Simple constructor
    pub fn new(
        provider: ExchangeRateProvider,
        failed_polls_before_invalidation: u32,
        store: S,
    ) -> Self {
        ExchangeRateFetcher {
            provider,
            consecutive_failed_polls: Arc::new(AtomicU32::new(0)),
            failed_polls_before_invalidation,
            store,
            client: Client::new(),
        }
    }

    /// Spawns a future which calls [`self.update_rates()`](./struct.ExchangeRateFetcher.html#method.update_rates) every `interval`
    pub fn spawn_interval(self, interval: Duration) {
        debug!(
            "Starting interval to poll exchange rate provider: {:?} for rates",
            self.provider
        );
        let interval = async move {
            let mut interval = tokio::time::interval(interval);
            loop {
                interval.tick().await;
                // Ignore errors so that they don't cause the Interval to stop
                let _ = self.update_rates().await;
            }
        };
        tokio::spawn(interval);
    }

    /// Calls the proper exchange rate provider
    async fn fetch_rates(&self) -> Result<HashMap<String, f64>, ()> {
        match self.provider {
            ExchangeRateProvider::CryptoCompare(ref api_key) => {
                query_cryptocompare(&self.client, api_key).await
            }
            ExchangeRateProvider::CoinCap => query_coincap(&self.client).await,
        }
    }

    /// Gets the exchange rates and proceeds to update the store with the newly polled values
    async fn update_rates(&self) -> Result<(), ()> {
        let consecutive_failed_polls = self.consecutive_failed_polls.clone();
        let consecutive_failed_polls_zeroer = consecutive_failed_polls.clone();
        let failed_polls_before_invalidation = self.failed_polls_before_invalidation;
        let store = self.store.clone();
        let store_clone = self.store.clone();
        let provider = self.provider.clone();
        let mut rates = self.fetch_rates()
            .map_err(move |_| {
                // Note that a race between the read on this line and the check on the line after
                // is quite unlikely as long as the interval between polls is reasonable.
                let failed_polls = consecutive_failed_polls.fetch_add(1, Ordering::Relaxed);
                if failed_polls < failed_polls_before_invalidation {
                    warn!("Failed to update exchange rates (previous consecutive failed attempts: {})", failed_polls);
                } else {
                    error!("Failed to update exchange rates (previous consecutive failed attempts: {}), removing old rates for safety", failed_polls);
                    // Clear out all of the old rates
                    if store.set_exchange_rates(HashMap::new()).is_err() {
                        error!("Failed to clear exchange rates cache after exchange rates server became unresponsive; panicking");
                        panic!("Failed to clear exchange rates cache after exchange rates server became unresponsive");
                    }
                }
            }).await?;

        trace!("Fetched exchange rates: {:?}", rates);
        let num_rates = rates.len();
        rates.insert("USD".to_string(), 1.0);
        if store_clone.set_exchange_rates(rates).is_ok() {
            // Reset our invalidation counter
            consecutive_failed_polls_zeroer.store(0, Ordering::Relaxed);
            debug!("Updated {} exchange rates from {:?}", num_rates, provider);
            Ok(())
        } else {
            error!("Error setting exchange rates in store");
            Err(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use interledger_errors::AddressStoreError;
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
