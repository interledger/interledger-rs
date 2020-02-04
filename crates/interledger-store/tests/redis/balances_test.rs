use super::{fixtures::*, store_helpers::*};

use interledger_api::NodeStore;
use interledger_packet::Address;
use interledger_service::Account as AccountTrait;
use interledger_service::{AccountStore, Username};
use interledger_service_util::BalanceStore;
use redis_crate::AsyncCommands;
use std::str::FromStr;
use uuid::Uuid;

#[tokio::test]
async fn get_balance() {
    let (store, context, _accs) = test_store().await.unwrap();
    let account_id = Uuid::new_v4();
    let mut connection = context.async_connection().await.unwrap();
    let _: redis_crate::Value = connection
        .hset_multiple(
            format!("accounts:{}", account_id),
            &[("balance", 600u64), ("prepaid_amount", 400u64)],
        )
        .await
        .unwrap();

    let balance = store.get_balance(account_id).await.unwrap();
    assert_eq!(balance, 1000);
}

#[tokio::test]
async fn prepare_then_fulfill_with_settlement() {
    let (store, _context, accs) = test_store().await.unwrap();
    let accounts = store
        .get_accounts(vec![accs[0].id(), accs[1].id()])
        .await
        .unwrap();
    let account0_id = accounts[0].id();
    let account1_id = accounts[1].id();
    // reduce account 0's balance by 100
    store
        .update_balances_for_prepare(account0_id, 100)
        .await
        .unwrap();
    let balance0 = store.get_balance(account0_id).await.unwrap();
    let balance1 = store.get_balance(account1_id).await.unwrap();
    assert_eq!(balance0, -100);
    assert_eq!(balance1, 0);

    store
        .update_balances_for_fulfill(account1_id, 100)
        .await
        .unwrap();
    let balance0 = store.get_balance(account0_id).await.unwrap();
    let balance1 = store.get_balance(account1_id).await.unwrap();
    assert_eq!(balance0, -100);
    assert_eq!(balance1, -1000);

    // prepare/fulfill functions work even with a dropped socket if the amount is 0
    drop(_context);
    store
        .update_balances_for_prepare(account1_id, 0)
        .await
        .unwrap();
    store
        .update_balances_for_fulfill(account1_id, 0)
        .await
        .unwrap();

    // but they error of course if they need to use the socket
    let err = store
        .update_balances_for_prepare(account1_id, 1)
        .await
        .unwrap_err();
    assert_eq!(err.to_string(), "Broken pipe (os error 32)");
    let err = store
        .update_balances_for_fulfill(account1_id, 1)
        .await
        .unwrap_err();
    // os error 32 only appears the first time
    assert_eq!(err.to_string(), "broken pipe");
}

#[tokio::test]
async fn process_fulfill_no_settle_to() {
    // account without a settle_to
    let acc = {
        let mut acc = ACCOUNT_DETAILS_1.clone();
        acc.username = Username::from_str("charlie").unwrap();
        acc.ilp_address = Some(Address::from_str("example.charlie").unwrap());
        acc.ilp_over_http_incoming_token = None;
        acc.ilp_over_http_outgoing_token = None;
        acc.ilp_over_btp_incoming_token = None;
        acc.settle_to = None;
        acc
    };
    let (store, _context, _accs) = test_store().await.unwrap();
    let account = store.insert_account(acc).await.unwrap();
    let id = account.id();
    let (balance, amount_to_settle) = store.update_balances_for_fulfill(id, 100).await.unwrap();
    assert_eq!(balance, 100);
    assert_eq!(amount_to_settle, 0);
}

#[tokio::test]
async fn process_fulfill_settle_to_over_threshold() {
    // account misconfigured with settle_to >= settle_threshold does not get settlements
    let acc = {
        let mut acc = ACCOUNT_DETAILS_1.clone();
        acc.username = Username::from_str("charlie").unwrap();
        acc.ilp_address = Some(Address::from_str("example.b").unwrap());
        acc.settle_to = Some(101);
        acc.settle_threshold = Some(100);
        acc.ilp_over_http_incoming_token = None;
        acc.ilp_over_http_outgoing_token = None;
        acc.ilp_over_btp_incoming_token = None;
        acc
    };
    let (store, _context, _accs) = test_store().await.unwrap();
    let acc = store.insert_account(acc).await.unwrap();
    let id = acc.id();
    let (balance, amount_to_settle) = store.update_balances_for_fulfill(id, 1000).await.unwrap();
    assert_eq!(balance, 1000);
    assert_eq!(amount_to_settle, 0);
}

#[tokio::test]
async fn process_fulfill_ok() {
    // account with settle to = 0 (not falsy) with settle_threshold > 0, gets settlements
    let acc = {
        let mut acc = ACCOUNT_DETAILS_1.clone();
        acc.username = Username::from_str("charlie").unwrap();
        acc.ilp_address = Some(Address::from_str("example.c").unwrap());
        acc.settle_to = Some(0);
        acc.settle_threshold = Some(100);
        acc.ilp_over_http_incoming_token = None;
        acc.ilp_over_http_outgoing_token = None;
        acc.ilp_over_btp_incoming_token = None;
        acc
    };
    let (store, _context, _accs) = test_store().await.unwrap();
    let account = store.insert_account(acc).await.unwrap();
    let id = account.id();
    let (balance, amount_to_settle) = store.update_balances_for_fulfill(id, 101).await.unwrap();
    assert_eq!(balance, 0);
    assert_eq!(amount_to_settle, 101);
}

#[tokio::test]
async fn prepare_then_reject() {
    let (store, _context, accs) = test_store().await.unwrap();
    let acc0 = accs[0].id();
    let acc1 = accs[1].id();
    store.update_balances_for_prepare(acc0, 100).await.unwrap();
    let balance0 = store.get_balance(acc0).await.unwrap();
    let balance1 = store.get_balance(acc1).await.unwrap();
    assert_eq!(balance0, -100);
    assert_eq!(balance1, 0);
    store.update_balances_for_reject(acc0, 100).await.unwrap();
    let balance0 = store.get_balance(acc0).await.unwrap();
    let balance1 = store.get_balance(acc1).await.unwrap();
    assert_eq!(balance0, 0);
    assert_eq!(balance1, 0);
}

#[tokio::test]
async fn enforces_minimum_balance() {
    let (store, _context, accs) = test_store().await.unwrap();
    let id = accs[0].id();
    let err = store
        .update_balances_for_prepare(id, 10000)
        .await
        .unwrap_err();
    let expected = format!("Incoming prepare of 10000 would bring account {} under its minimum balance. Current balance: 0, min balance: -1000", id);
    assert!(err.to_string().contains(&expected));
}

#[tokio::test]
// Prepare and Fulfill a packet for 100 units from Account 0 to Account 1
// Then, Prepare and Fulfill a packet for 80 units from Account 1 to Account 0
async fn netting_fulfilled_balances() {
    let (store, _context, accs) = test_store().await.unwrap();
    let acc = store
        .insert_account(ACCOUNT_DETAILS_2.clone())
        .await
        .unwrap();
    let account0 = accs[0].id();
    let account1 = acc.id();

    // decrement account 0 by 100
    store
        .update_balances_for_prepare(account0, 100)
        .await
        .unwrap();
    // increment account 1 by 100
    store
        .update_balances_for_fulfill(account1, 100)
        .await
        .unwrap();

    // decrement account 1 by 80
    store
        .update_balances_for_prepare(account1, 80)
        .await
        .unwrap();
    // increment account 0 by 80
    store
        .update_balances_for_fulfill(account0, 80)
        .await
        .unwrap();

    let balance0 = store.get_balance(account0).await.unwrap();
    let balance1 = store.get_balance(account1).await.unwrap();
    assert_eq!(balance0, -20);
    assert_eq!(balance1, 20);
}
