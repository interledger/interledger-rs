use super::store_helpers::*;
use futures::future::Future;
use interledger_service_util::ExchangeRateStore;

#[test]
fn set_rates() {
    block_on(test_store().and_then(|(store, context, _accs)| {
        let store_clone = store.clone();
        let rates = store.get_exchange_rates(&["ABC", "XYZ"]);
        assert!(rates.is_err());
        store
            .set_exchange_rates(
                [("ABC".to_string(), 500.0), ("XYZ".to_string(), 0.005)]
                    .iter()
                    .cloned()
                    .collect(),
            )
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
