use futures::{future::err, Future};
use interledger_packet::{Address, ErrorCode, Fulfill, Reject, RejectBuilder};
use interledger_service::*;
use interledger_settlement::{Convert, ConvertDetails};
use log::{error, trace};
use std::marker::PhantomData;

pub trait ExchangeRateStore {
    fn get_exchange_rates(&self, asset_codes: &[&str]) -> Result<Vec<f64>, ()>;
}

/// # Exchange Rates Service
///
/// Responsible for getting the exchange rates for the two assets in the outgoing request (`request.from.asset_code`, `request.to.asset_code`).
/// Requires a `ExchangeRateStore`
#[derive(Clone)]
pub struct ExchangeRateService<S, O, A> {
    ilp_address: Address,
    store: S,
    next: O,
    account_type: PhantomData<A>,
}

impl<S, O, A> ExchangeRateService<S, O, A>
where
    S: ExchangeRateStore,
    O: OutgoingService<A>,
    A: Account,
{
    pub fn new(ilp_address: Address, store: S, next: O) -> Self {
        ExchangeRateService {
            ilp_address,
            store,
            next,
            account_type: PhantomData,
        }
    }
}

impl<S, O, A> OutgoingService<A> for ExchangeRateService<S, O, A>
where
    // TODO can we make these non-'static?
    S: ExchangeRateStore + Clone + Send + Sync + 'static,
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
        if request.prepare.amount() > 0 {
            let rate: f64 = if request.from.asset_code() == request.to.asset_code() {
                1f64
            } else if let Ok(rates) = self
                .store
                .get_exchange_rates(&[&request.from.asset_code(), &request.to.asset_code()])
            {
                rates[1] / rates[0]
            } else {
                error!(
                    "No exchange rates available for assets: {}, {}",
                    request.from.asset_code(),
                    request.to.asset_code()
                );
                return Box::new(err(RejectBuilder {
                    // Unreachable doesn't seem to be the correct code here.
                    // If the pair was not found, shouldn't we have a unique error code
                    // for that such as `ErrorCode::F10_PAIRNOTFOUND` ?
                    // Timeout should still apply we if we add a timeout
                    // error in the get_exchange_rate call
                    code: ErrorCode::F02_UNREACHABLE,
                    message: format!(
                        "No exchange rate available from asset: {} to: {}",
                        request.from.asset_code(),
                        request.to.asset_code()
                    )
                    .as_bytes(),
                    triggered_by: Some(&self.ilp_address),
                    data: &[],
                }
                .build()));
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
                            triggered_by: Some(&self.ilp_address),
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
                        triggered_by: Some(&self.ilp_address),
                        data: &[],
                    }
                    .build()));
                }
            }
        }

        Box::new(self.next.send_request(request))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::{future::ok, Future};
    use interledger_packet::{Address, FulfillBuilder, PrepareBuilder};
    use interledger_service::{outgoing_service_fn, Account};
    use std::collections::HashMap;
    use std::str::FromStr;
    use std::{
        sync::{Arc, Mutex},
        time::SystemTime,
    };

    #[test]
    fn exchange_rate_ok() {
        let ret = exchange_rate(100, 1, 1.0, 1, 2.0);
        assert_eq!(ret.1[0].prepare.amount(), 200);

        let ret = exchange_rate(1_000_000, 1, 3.0, 1, 2.0);
        assert_eq!(ret.1[0].prepare.amount(), 666_666);
    }

    #[test]
    fn exchange_conversion_error() {
        // rejects f64 that does not fit in u64
        let ret = exchange_rate(std::u64::MAX, 1, 1.0, 1, 2.0);
        let reject = ret.0.unwrap_err();
        assert_eq!(reject.code(), ErrorCode::F08_AMOUNT_TOO_LARGE);
        assert!(reject.message().starts_with(b"Could not cast"));

        // `Convert` errored
        let ret = exchange_rate(std::u64::MAX, 1, 1.0, 255, std::f64::MAX);
        let reject = ret.0.unwrap_err();
        assert_eq!(reject.code(), ErrorCode::F08_AMOUNT_TOO_LARGE);
        assert!(reject.message().starts_with(b"Could not convert"));
    }

    // Instantiates an exchange rate service and returns the fulfill/reject
    // packet and the outgoing request after performing an asset conversion
    fn exchange_rate(
        amount: u64,
        scale1: u8,
        rate1: f64,
        scale2: u8,
        rate2: f64,
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
        let mut service = test_service(rate1, rate2, outgoing);
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

    impl Account for TestAccount {
        type AccountId = u64;

        fn id(&self) -> u64 {
            0
        }

        fn asset_code(&self) -> &str {
            &self.asset_code
        }

        fn asset_scale(&self) -> u8 {
            self.asset_scale
        }

        fn client_address(&self) -> &Address {
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
    }

    fn test_store(rate1: f64, rate2: f64) -> TestStore {
        let mut rates = HashMap::new();
        rates.insert(vec!["ABC".to_owned(), "XYZ".to_owned()], (rate1, rate2));
        TestStore { rates }
    }

    fn test_service(
        rate1: f64,
        rate2: f64,
        handler: impl OutgoingService<TestAccount> + Clone + Send + Sync,
    ) -> ExchangeRateService<
        TestStore,
        impl OutgoingService<TestAccount> + Clone + Send + Sync,
        TestAccount,
    > {
        let store = test_store(rate1, rate2);
        ExchangeRateService::new(Address::from_str("example.bob").unwrap(), store, handler)
    }

}
