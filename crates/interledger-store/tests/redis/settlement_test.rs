use super::store_helpers::*;
use bytes::Bytes;

use http::StatusCode;
use interledger_api::NodeStore;
use interledger_service::{Account, AccountStore};
use interledger_service_util::BalanceStore;
use interledger_settlement::core::{
    idempotency::{IdempotentData, IdempotentStore},
    types::{LeftoversStore, SettlementAccount, SettlementStore},
};
use num_bigint::BigUint;
use once_cell::sync::Lazy;
use redis_crate::cmd;
use redis_crate::AsyncCommands;
use url::Url;
use uuid::Uuid;

static IDEMPOTENCY_KEY: Lazy<String> = Lazy::new(|| String::from("AJKJNUjM0oyiAN46"));

#[tokio::test]
async fn saves_gets_clears_uncredited_settlement_amount_properly() {
    let (store, _context, _accs) = test_store().await.unwrap();
    let amounts: Vec<(BigUint, u8)> = vec![
        (BigUint::from(5u32), 11),   // 5
        (BigUint::from(855u32), 12), // 905
        (BigUint::from(1u32), 10),   // 1005 total
    ];
    let acc = Uuid::new_v4();
    for a in amounts {
        store
            .save_uncredited_settlement_amount(acc, a)
            .await
            .unwrap();
    }
    let ret = store
        .load_uncredited_settlement_amount(acc, 9u8)
        .await
        .unwrap();
    // 1 uncredited unit for scale 9
    assert_eq!(ret, BigUint::from(1u32));
    // rest should be in the leftovers store
    let ret = store.get_uncredited_settlement_amount(acc).await.unwrap();
    // 1 uncredited unit for scale 9
    assert_eq!(ret, (BigUint::from(5u32), 12));

    // clears uncredited amount
    store.clear_uncredited_settlement_amount(acc).await.unwrap();
    let ret = store.get_uncredited_settlement_amount(acc).await.unwrap();
    assert_eq!(ret, (BigUint::from(0u32), 0));
}

#[tokio::test]
async fn saves_and_loads_idempotency_key_data_properly() {
    let (store, _context, _) = test_store().await.unwrap();
    let input_hash: [u8; 32] = Default::default();
    store
        .save_idempotent_data(
            IDEMPOTENCY_KEY.clone(),
            input_hash,
            StatusCode::OK,
            Bytes::from("TEST"),
        )
        .await
        .unwrap();
    let data1 = store
        .load_idempotent_data(IDEMPOTENCY_KEY.clone())
        .await
        .unwrap();
    assert_eq!(
        data1.unwrap(),
        IdempotentData::new(StatusCode::OK, Bytes::from("TEST"), input_hash)
    );

    let data2 = store
        .load_idempotent_data("asdf".to_string())
        .await
        .unwrap();
    assert!(data2.is_none());
}

#[tokio::test]
async fn idempotent_settlement_calls() {
    let (store, _context, accs) = test_store().await.unwrap();
    let id = accs[0].id();
    store
        .update_balance_for_incoming_settlement(id, 100, Some(IDEMPOTENCY_KEY.clone()))
        .await
        .unwrap();
    let balance = store.get_balance(id).await.unwrap();
    assert_eq!(balance, 100);

    store
        .update_balance_for_incoming_settlement(
            id,
            100,
            Some(IDEMPOTENCY_KEY.clone()), // Reuse key to make idempotent request.
        )
        .await
        .unwrap();
    let balance = store.get_balance(id).await.unwrap();
    // Since it's idempotent there
    // will be no state update.
    // Otherwise it'd be 200 (100 + 100)
    assert_eq!(balance, 100);
}

#[tokio::test]
async fn credits_prepaid_amount() {
    let (store, context, accs) = test_store().await.unwrap();
    let id = accs[0].id();
    let mut conn = context.async_connection().await.unwrap();
    store
        .update_balance_for_incoming_settlement(id, 100, Some(IDEMPOTENCY_KEY.clone()))
        .await
        .unwrap();
    let (balance, prepaid_amount): (i64, i64) = cmd("HMGET")
        .arg(format!("accounts:{}", id))
        .arg("balance")
        .arg("prepaid_amount")
        .query_async(&mut conn)
        .await
        .unwrap();
    assert_eq!(balance, 0);
    assert_eq!(prepaid_amount, 100);
}

#[tokio::test]
async fn credits_balance_owed() {
    let (store, context, accs) = test_store().await.unwrap();
    let id = accs[0].id();
    let mut connection = context.shared_async_connection().await.unwrap();
    let _balance: i64 = connection
        .hset(format!("accounts:{}", id), "balance", -200i64)
        .await
        .unwrap();
    // since we have some balance already, it will try to
    // increase the balance field
    store
        .update_balance_for_incoming_settlement(id, 100, Some(IDEMPOTENCY_KEY.clone()))
        .await
        .unwrap();

    let (balance, prepaid_amount): (i64, i64) = connection
        .hget(format!("accounts:{}", id), &["balance", "prepaid_amount"])
        .await
        .unwrap();
    assert_eq!(balance, -100);
    assert_eq!(prepaid_amount, 0);
}

#[tokio::test]
async fn clears_balance_owed() {
    let (store, context, accs) = test_store().await.unwrap();
    let id = accs[0].id();
    let mut connection = context.shared_async_connection().await.unwrap();
    let _balance: i64 = connection
        .hset(format!("accounts:{}", id), "balance", -100i64)
        .await
        .unwrap();
    store
        .update_balance_for_incoming_settlement(id, 100, Some(IDEMPOTENCY_KEY.clone()))
        .await
        .unwrap();
    let (balance, prepaid_amount): (i64, i64) = connection
        .hget(format!("accounts:{}", id), &["balance", "prepaid_amount"])
        .await
        .unwrap();
    assert_eq!(balance, 0);
    assert_eq!(prepaid_amount, 0);
}

#[tokio::test]
async fn clears_balance_owed_and_puts_remainder_as_prepaid() {
    let (store, context, accs) = test_store().await.unwrap();
    let id = accs[0].id();
    let mut connection = context.shared_async_connection().await.unwrap();
    let _balance: i64 = connection
        .hset(format!("accounts:{}", id), "balance", -40i64)
        .await
        .unwrap();
    store
        .update_balance_for_incoming_settlement(id, 100, Some(IDEMPOTENCY_KEY.clone()))
        .await
        .unwrap();
    let (balance, prepaid_amount): (i64, i64) = connection
        .hget(format!("accounts:{}", id), &["balance", "prepaid_amount"])
        .await
        .unwrap();
    assert_eq!(balance, 0);
    assert_eq!(prepaid_amount, 60);
}

#[tokio::test]
async fn loads_globally_configured_settlement_engine_url() {
    let (store, _context, accs) = test_store().await.unwrap();
    assert!(accs[0].settlement_engine_details().is_some());
    assert!(accs[1].settlement_engine_details().is_none());
    let account_ids = vec![accs[0].id(), accs[1].id()];
    let accounts = store.get_accounts(account_ids.clone()).await.unwrap();
    assert!(accounts[0].settlement_engine_details().is_some());
    assert!(accounts[1].settlement_engine_details().is_none());

    store
        .set_settlement_engines(vec![
            (
                "ABC".to_string(),
                Url::parse("http://settle-abc.example").unwrap(),
            ),
            (
                "XYZ".to_string(),
                Url::parse("http://settle-xyz.example").unwrap(),
            ),
        ])
        .await
        .unwrap();
    let accounts = store.get_accounts(account_ids).await.unwrap();
    // It should not overwrite the one that was individually configured
    assert_eq!(
        accounts[0]
            .settlement_engine_details()
            .unwrap()
            .url
            .as_str(),
        "http://settlement.example/"
    );

    // It should set the URL for the account that did not have one configured
    assert!(accounts[1].settlement_engine_details().is_some());
    assert_eq!(
        accounts[1]
            .settlement_engine_details()
            .unwrap()
            .url
            .as_str(),
        "http://settle-abc.example/"
    );
}
