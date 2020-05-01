use futures::TryFutureExt;
use interledger_errors::ExchangeRateStoreError;
use reqwest::Client;
use secrecy::SecretString;
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio;
use tracing::{debug, error, trace, warn};

mod cryptocompare;

mod coincap;

pub trait ExchangeRateStore: Clone {
    // TODO we may want to make this async if/when we use pubsub to broadcast
    // rate changes to different instances of a horizontally-scalable node
    fn set_exchange_rates(&self, rates: HashMap<String, f64>)
        -> Result<(), ExchangeRateStoreError>;

    fn get_exchange_rates(&self, asset_codes: &[&str]) -> Result<Vec<f64>, ExchangeRateStoreError>;

    // TODO should this be on the API instead? That's where it's actually used
    // TODO should we combine this method with get_exchange_rates?
    // The downside of doing that is in this case we want a HashMap with owned values
    // (so that we don't accidentally lock up the RwLock on the store's exchange_rates)
    // but in the normal case of getting the rate between two assets, we don't want to
    // copy all the rate data
    fn get_all_exchange_rates(&self) -> Result<HashMap<String, f64>, ExchangeRateStoreError>;
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
                cryptocompare::query_cryptocompare(&self.client, api_key).await
            }
            ExchangeRateProvider::CoinCap => coincap::query_coincap(&self.client).await,
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
        #[allow(clippy::cognitive_complexity)]
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
