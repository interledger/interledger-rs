use super::store_helpers::*;

use interledger_rates::ExchangeRateStore;

#[tokio::test]
async fn set_rates() {
    let (store, _context, _) = test_store().await.unwrap();
    let rates = store.get_exchange_rates(&["ABC", "XYZ"]);
    assert!(rates.is_err());
    store
        .set_exchange_rates(
            [("ABC".to_string(), 500.0), ("XYZ".to_string(), 0.005)]
                .iter()
                .cloned()
                .collect(),
        )
        .unwrap();

    let rates = store.get_exchange_rates(&["XYZ", "ABC"]).unwrap();
    assert_eq!(rates[0].to_string(), "0.005");
    assert_eq!(rates[1].to_string(), "500");
}
