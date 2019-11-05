use super::exchange_rate_providers::*;
use futures::{
    future::{err, Either},
    Future, Stream,
};
use interledger_packet::{ErrorCode, Fulfill, Reject, RejectBuilder};
use interledger_service::*;
// TODO remove the dependency on interledger_settlement, that doesn't really make sense for this minor import
use interledger_settlement::{Convert, ConvertDetails};
use log::{debug, error, trace, warn};
use reqwest::r#async::Client;
use secrecy::SecretString;
use serde::Deserialize;
use std::{
    collections::HashMap,
    marker::PhantomData,
    sync::{
        atomic::{AtomicU32, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};
use tokio::{executor::spawn, timer::Interval};

// TODO should this whole file be moved to its own crate?

pub trait ExchangeRateStore: Clone {
    // TODO we may want to make this async if/when we use pubsub to broadcast
    // rate changes to different instances of a horizontally-scalable node
    fn set_exchange_rates(&self, rates: HashMap<String, f64>) -> Result<(), ()>;

    fn get_exchange_rates(&self, asset_codes: &[&str]) -> Result<Vec<f64>, ()>;

    // TODO should this be on the API instead? That's where it's actually used
    // TODO should we combine this method with get_exchange_rates?
    // The downside of doing that is in this case we want a HashMap with owned values
    // (so that we don't accidentally lock up the RwLock on the store's exchange_rates)
    // but in the normal case of getting the rate between two assets, we don't want to
    // copy all the rate data
    fn get_all_exchange_rates(&self) -> Result<HashMap<String, f64>, ()>;
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

impl<S, O, A> OutgoingService<A> for ExchangeRateService<S, O, A>
where
    // TODO can we make these non-'static?
    S: AddressStore + ExchangeRateStore + Clone + Send + Sync + 'static,
    O: OutgoingService<A> + Send + Clone + 'static,
    A: Account + Sync + 'static,
{
    type Future = BoxedIlpFuture;

    /// On send request:
    /// 1. If the prepare packet's amount is 0, it just forwards
    /// 1. Retrieves the exchange rate from the store (the store independently is responsible for polling the rates)
    ///     - return reject if the call to the store fails
    /// 1. Calculates the exchange rate AND scales it up/down depending on how many decimals each asset requires
    /// 1. Updates the amount in the prepare packet and forwards it
    fn send_request(
        &mut self,
        mut request: OutgoingRequest<A>,
    ) -> Box<dyn Future<Item = Fulfill, Error = Reject> + Send> {
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
                return Box::new(err(RejectBuilder {
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
                .build()));
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
                        return Box::new(err(RejectBuilder {
                            code: ErrorCode::F08_AMOUNT_TOO_LARGE,
                            message: format!(
                                "Could not cast outgoing amount to u64 {}",
                                outgoing_amount,
                            )
                            .as_bytes(),
                            triggered_by: Some(&ilp_address),
                            data: &[],
                        }
                        .build()));
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
                    return Box::new(err(RejectBuilder {
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
                    .build()));
                }
            }
        }

        Box::new(self.next.send_request(request))
    }
}

/// This determines which external API service to poll for exchange rates.
#[derive(Debug, Clone, Deserialize)]
pub enum ExchangeRateProvider {
    /// Use the [CoinCap] API.
    ///
    /// Note that when configured with YAML, this MUST be specified as
    /// "CoinCap", not "coin_cap".
    ///
    /// [CoinCap]: https://coincap.io/
    #[serde(alias = "coin_cap", alias = "coincap", alias = "Coincap")]
    CoinCap,
    /// Use the [CryptoCompare] API. Note this service requires an
    /// API key (but the free tier supports 100,000 requests / month at the
    /// time of writing).
    ///
    /// Note that when configured with YAML, this MUST be specified as
    /// "CryptoCompare", not "crypto_compare".
    ///
    /// [CryptoCompare]: https://cryptocompare.com
    #[serde(
        alias = "crypto_compare",
        alias = "cryptocompare",
        alias = "Cryptocompare"
    )]
    CryptoCompare(SecretString),
}

/// Poll exchange rate providers for the current exchange rates
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

    pub fn fetch_on_interval(self, interval: Duration) -> impl Future<Item = (), Error = ()> {
        debug!(
            "Starting interval to poll exchange rate provider: {:?} for rates",
            self.provider
        );
        Interval::new(Instant::now(), interval)
            .map_err(|err| {
                error!(
                    "Interval error, no longer fetching exchange rates: {:?}",
                    err
                );
            })
            .for_each(move |_| {
                self.update_rates().then(|_| {
                    // Ignore errors so that they don't cause the Interval to stop
                    Ok(())
                })
            })
    }

    pub fn spawn_interval(self, interval: Duration) {
        spawn(self.fetch_on_interval(interval));
    }

    fn fetch_rates(&self) -> impl Future<Item = HashMap<String, f64>, Error = ()> {
        match self.provider {
            ExchangeRateProvider::CryptoCompare(ref api_key) => {
                Either::A(query_cryptocompare(&self.client, api_key))
            }
            ExchangeRateProvider::CoinCap => Either::B(query_coincap(&self.client)),
        }
    }

    fn update_rates(&self) -> impl Future<Item = (), Error = ()> {
        let consecutive_failed_polls = self.consecutive_failed_polls.clone();
        let consecutive_failed_polls_zeroer = consecutive_failed_polls.clone();
        let failed_polls_before_invalidation = self.failed_polls_before_invalidation;
        let store = self.store.clone();
        let store_clone = self.store.clone();
        let provider = self.provider.clone();
        self.fetch_rates()
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
            })
            .and_then(move |mut rates| {
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
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::{future::ok, Future};
    use interledger_packet::{Address, FulfillBuilder, PrepareBuilder};
    use interledger_service::{outgoing_service_fn, Account};
    use lazy_static::lazy_static;
    use std::collections::HashMap;
    use std::str::FromStr;
    use std::{
        sync::{Arc, Mutex},
        time::SystemTime,
    };

    lazy_static! {
        pub static ref ALICE: Username = Username::from_str("alice").unwrap();
    }

    #[test]
    fn exchange_rate_ok() {
        // if `to` is worth $2, and `from` is worth 1, then they receive half
        // the amount of units
        let ret = exchange_rate(200, 1, 1.0, 1, 2.0, 0.0);
        assert_eq!(ret.1[0].prepare.amount(), 100);

        let ret = exchange_rate(1_000_000, 1, 3.0, 1, 2.0, 0.0);
        assert_eq!(ret.1[0].prepare.amount(), 1_500_000);
    }

    #[test]
    fn exchange_conversion_error() {
        // rejects f64 that does not fit in u64
        let ret = exchange_rate(std::u64::MAX, 1, 2.0, 1, 1.0, 0.0);
        let reject = ret.0.unwrap_err();
        assert_eq!(reject.code(), ErrorCode::F08_AMOUNT_TOO_LARGE);
        assert!(reject.message().starts_with(b"Could not cast"));

        // `Convert` errored
        let ret = exchange_rate(std::u64::MAX, 1, std::f64::MAX, 255, 1.0, 0.0);
        let reject = ret.0.unwrap_err();
        assert_eq!(reject.code(), ErrorCode::F08_AMOUNT_TOO_LARGE);
        assert!(reject.message().starts_with(b"Could not convert"));
    }

    #[test]
    fn applies_spread() {
        let ret = exchange_rate(100, 1, 1.0, 1, 2.0, 0.01);
        assert_eq!(ret.1[0].prepare.amount(), 49);

        // Negative spread is unusual but possible
        let ret = exchange_rate(200, 1, 1.0, 1, 2.0, -0.01);
        assert_eq!(ret.1[0].prepare.amount(), 101);

        // Rounds down
        let ret = exchange_rate(4, 1, 1.0, 1, 2.0, 0.01);
        // this would've been 2, but it becomes 1.99 and gets rounded down to 1
        assert_eq!(ret.1[0].prepare.amount(), 1);

        // Spread >= 1 means the node takes everything
        let ret = exchange_rate(10_000_000_000, 1, 1.0, 1, 2.0, 1.0);
        assert_eq!(ret.1[0].prepare.amount(), 0);

        // Need to catch when spread > 1
        let ret = exchange_rate(10_000_000_000, 1, 1.0, 1, 2.0, 2.0);
        assert_eq!(ret.1[0].prepare.amount(), 0);
    }

    // Instantiates an exchange rate service and returns the fulfill/reject
    // packet and the outgoing request after performing an asset conversion
    fn exchange_rate(
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
            Box::new(ok(FulfillBuilder {
                fulfillment: &[0; 32],
                data: b"hello!",
            }
            .build()))
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
            .wait();

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

    impl AddressStore for TestStore {
        /// Saves the ILP Address in the store's memory and database
        fn set_ilp_address(
            &self,
            _ilp_address: Address,
        ) -> Box<dyn Future<Item = (), Error = ()> + Send> {
            unimplemented!()
        }

        fn clear_ilp_address(&self) -> Box<dyn Future<Item = (), Error = ()> + Send> {
            unimplemented!()
        }

        /// Get's the store's ilp address from memory
        fn get_ilp_address(&self) -> Address {
            Address::from_str("example.connector").unwrap()
        }
    }

    impl Account for TestAccount {
        type AccountId = u64;

        fn id(&self) -> u64 {
            0
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
        fn get_exchange_rates(&self, asset_codes: &[&str]) -> Result<Vec<f64>, ()> {
            let mut ret = Vec::new();
            let key = vec![asset_codes[0].to_owned(), asset_codes[1].to_owned()];
            let v = self.rates.get(&key);
            if let Some(v) = v {
                ret.push(v.0);
                ret.push(v.1);
            } else {
                return Err(());
            }
            Ok(ret)
        }

        fn set_exchange_rates(&self, _rates: HashMap<String, f64>) -> Result<(), ()> {
            unimplemented!()
        }

        fn get_all_exchange_rates(&self) -> Result<HashMap<String, f64>, ()> {
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
