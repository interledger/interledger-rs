use super::{fixtures::*, store_helpers::*};
use futures::future::join_all;
use interledger_service::AddressStore;
use interledger_service_util::{RateLimitError, RateLimitStore};
use interledger_store::account::Account;
use uuid::Uuid;

#[tokio::test]
async fn rate_limits_number_of_packets() {
    let (store, _context, _) = test_store().await.unwrap();
    let account = Account::try_from(
        Uuid::new_v4(),
        ACCOUNT_DETAILS_0.clone(),
        store.get_ilp_address(),
    )
    .unwrap();
    let results = join_all(vec![
        store.clone().apply_rate_limits(account.clone(), 10),
        store.clone().apply_rate_limits(account.clone(), 10),
        store.clone().apply_rate_limits(account.clone(), 10),
    ])
    .await;
    // The first 2 calls succeed, while the 3rd one hits the rate limit error
    // because the account is only allowed 2 packets per minute
    assert_eq!(
        results,
        vec![Ok(()), Ok(()), Err(RateLimitError::PacketLimitExceeded)]
    );
}

#[tokio::test]
async fn limits_amount_throughput() {
    let (store, _context, _) = test_store().await.unwrap();
    let account = Account::try_from(
        Uuid::new_v4(),
        ACCOUNT_DETAILS_1.clone(),
        store.get_ilp_address(),
    )
    .unwrap();
    let results = join_all(vec![
        store.clone().apply_rate_limits(account.clone(), 500),
        store.clone().apply_rate_limits(account.clone(), 500),
        store.clone().apply_rate_limits(account.clone(), 1),
    ])
    .await;
    // The first 2 calls succeed, while the 3rd one hits the rate limit error
    // because the account is only allowed 1000 units of currency per minute
    assert_eq!(
        results,
        vec![Ok(()), Ok(()), Err(RateLimitError::ThroughputLimitExceeded)]
    );
}

#[tokio::test]
async fn refunds_throughput_limit_for_rejected_packets() {
    let (store, _context, _) = test_store().await.unwrap();
    let account = Account::try_from(
        Uuid::new_v4(),
        ACCOUNT_DETAILS_1.clone(),
        store.get_ilp_address(),
    )
    .unwrap();

    join_all(vec![
        store.clone().apply_rate_limits(account.clone(), 500),
        store.clone().apply_rate_limits(account.clone(), 500),
    ])
    .await;

    // We refund the throughput limit once, meaning we can do 1 more call before
    // the error
    store
        .refund_throughput_limit(account.clone(), 500)
        .await
        .unwrap();
    store.apply_rate_limits(account.clone(), 500).await.unwrap();

    let result = store.apply_rate_limits(account.clone(), 1).await;
    assert_eq!(result.unwrap_err(), RateLimitError::ThroughputLimitExceeded);
}
