use super::{fixtures::*, store_helpers::*};
use futures::future::{join_all, Future};
use interledger_service::AddressStore;
use interledger_service_util::{RateLimitError, RateLimitStore};
use interledger_store::account::Account;
use uuid::Uuid;

#[test]
fn rate_limits_number_of_packets() {
    block_on(test_store().and_then(|(store, context, _accs)| {
        let account = Account::try_from(
            Uuid::new_v4(),
            ACCOUNT_DETAILS_0.clone(),
            store.get_ilp_address(),
        )
        .unwrap();
        join_all(vec![
            store.clone().apply_rate_limits(account.clone(), 10),
            store.clone().apply_rate_limits(account.clone(), 10),
            store.clone().apply_rate_limits(account.clone(), 10),
        ])
        .then(move |result| {
            assert!(result.is_err());
            assert_eq!(result.unwrap_err(), RateLimitError::PacketLimitExceeded);
            let _ = context;
            Ok(())
        })
    }))
    .unwrap()
}

#[test]
fn limits_amount_throughput() {
    block_on(test_store().and_then(|(store, context, _accs)| {
        let account = Account::try_from(
            Uuid::new_v4(),
            ACCOUNT_DETAILS_1.clone(),
            store.get_ilp_address(),
        )
        .unwrap();
        join_all(vec![
            store.clone().apply_rate_limits(account.clone(), 500),
            store.clone().apply_rate_limits(account.clone(), 500),
            store.clone().apply_rate_limits(account.clone(), 1),
        ])
        .then(move |result| {
            assert!(result.is_err());
            assert_eq!(result.unwrap_err(), RateLimitError::ThroughputLimitExceeded);
            let _ = context;
            Ok(())
        })
    }))
    .unwrap()
}

#[test]
fn refunds_throughput_limit_for_rejected_packets() {
    block_on(test_store().and_then(|(store, context, _accs)| {
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
        .map_err(|err| panic!(err))
        .and_then(move |_| {
            let store_clone = store.clone();
            let account_clone = account.clone();
            store
                .clone()
                .refund_throughput_limit(account.clone(), 500)
                .and_then(move |_| {
                    store
                        .clone()
                        .apply_rate_limits(account.clone(), 500)
                        .map_err(|err| panic!(err))
                })
                .and_then(move |_| {
                    store_clone
                        .apply_rate_limits(account_clone, 1)
                        .then(move |result| {
                            assert!(result.is_err());
                            assert_eq!(
                                result.unwrap_err(),
                                RateLimitError::ThroughputLimitExceeded
                            );
                            let _ = context;
                            Ok(())
                        })
                })
        })
    }))
    .unwrap()
}
