mod common;

use common::*;
use interledger_api::NodeStore;
use interledger_service_util::ExchangeRateStore;
use std::time::Duration;
use tokio_timer::sleep;

#[test]
fn set_rates() {
    block_on(test_store().and_then(|(store, context, _accs)| {
        let store_clone = store.clone();
        let rates = store.get_exchange_rates(&["ABC", "XYZ"]);
        assert!(rates.is_err());
        store
            .set_rates(vec![("ABC".to_string(), 500.0), ("XYZ".to_string(), 0.005)])
            .and_then(move |_| {
                let rates = store_clone.get_exchange_rates(&["XYZ", "ABC"]).unwrap();
                assert_eq!(rates[0].to_string(), "0.005");
                assert_eq!(rates[1].to_string(), "500");
                let _ = context;
                Ok(())
            })
    }))
    .unwrap();
}
#[test]
fn polls_for_rate_updates() {
    let context = TestContext::new();
    block_on(
        RedisStoreBuilder::new(context.get_client_connection_info(), [0; 32])
            .poll_interval(1)
            .connect()
            .and_then(|store| {
                assert!(store.get_exchange_rates(&["ABC", "XYZ"]).is_err());
                store
                    .clone()
                    .set_rates(vec![
                        ("ABC".to_string(), 0.5f64),
                        ("DEF".to_string(), 9_999_999_999.0f64),
                    ])
                    .and_then(|_| sleep(Duration::from_millis(10)).then(|_| Ok(())))
                    .and_then(move |_| {
                        assert_eq!(store.get_exchange_rates(&["ABC"]).unwrap(), vec![0.5]);
                        assert_eq!(
                            store.get_exchange_rates(&["ABC", "DEF"]).unwrap(),
                            vec![0.5, 9_999_999_999.0]
                        );
                        assert!(store.get_exchange_rates(&["ABC", "XYZ"]).is_err());
                        let _ = context;
                        Ok(())
                    })
            }),
    )
    .unwrap();
}
