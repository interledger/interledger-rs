use super::{fixtures::*, store_helpers::*};
use futures::future::{self, Either, Future};
use interledger_api::NodeStore;
use interledger_packet::Address;
use interledger_service::{Account as AccountTrait, AddressStore};
use interledger_service::{AccountStore, Username};
use interledger_service_util::BalanceStore;
use interledger_store::account::Account;
use std::str::FromStr;
use uuid::Uuid;

#[tokio::test]
async fn get_balance() {
    let (store, context, _accs) = test_store().await.unwrap();
    let account_id = Uuid::new_v4();
    let mut connection = context.async_connection().await.unwrap();
    let _: redis_crate::Value = redis_crate::cmd("HMSET")
        .arg(format!("accounts:{}", account_id))
        .arg("balance")
        .arg(600u64)
        .arg("prepaid_amount")
        .arg(400u64)
        .query_async(&mut connection)
        .await
        .unwrap();
    let account = Account::try_from(
        account_id,
        ACCOUNT_DETAILS_0.clone(),
        store.get_ilp_address(),
    )
    .unwrap();
    let balance = store.get_balance(account).await.unwrap();
    assert_eq!(balance, 1000);
}

#[tokio::test]
async fn prepare_then_fulfill_with_settlement() {
    let (store, _context, accs) = test_store().await.unwrap();
    let accounts = store
        .get_accounts(vec![accs[0].id(), accs[1].id()])
        .await
        .unwrap();
    let account0 = accounts[0].clone();
    let account1 = accounts[1].clone();
    // reduce account 0's balance by 100
    store
        .update_balances_for_prepare(account0.clone(), 100)
        .await
        .unwrap();
    // TODO:Can we make get_balance take a reference to the account?
    // Even better, we should make it just take the account uid/username!
    let balance0 = store.get_balance(account0.clone()).await.unwrap();
    let balance1 = store.get_balance(account1.clone()).await.unwrap();
    assert_eq!(balance0, -100);
    assert_eq!(balance1, 0);

    // Account 1 hits the settlement limit (?) TODO
    store
        .update_balances_for_fulfill(account1.clone(), 100)
        .await
        .unwrap();
    let balance0 = store.get_balance(account0).await.unwrap();
    let balance1 = store.get_balance(account1).await.unwrap();
    assert_eq!(balance0, -100);
    assert_eq!(balance1, -1000);
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
    let accounts = store.get_accounts(vec![id]).await.unwrap();
    let acc = accounts[0].clone();
    let (balance, amount_to_settle) = store
        .update_balances_for_fulfill(acc.clone(), 100)
        .await
        .unwrap();
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
    let accounts = store.get_accounts(vec![id]).await.unwrap();
    let acc = accounts[0].clone();
    let (balance, amount_to_settle) = store
        .update_balances_for_fulfill(acc.clone(), 1000)
        .await
        .unwrap();
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
    let accounts = store.get_accounts(vec![id]).await.unwrap();
    let acc = accounts[0].clone();
    let (balance, amount_to_settle) = store
        .update_balances_for_fulfill(acc.clone(), 101)
        .await
        .unwrap();
    assert_eq!(balance, 0);
    assert_eq!(amount_to_settle, 101);
}

#[tokio::test]
async fn prepare_then_reject() {
    let (store, _context, accs) = test_store().await.unwrap();
    let accounts = store
        .get_accounts(vec![accs[0].id(), accs[1].id()])
        .await
        .unwrap();
    let account0 = accounts[0].clone();
    let account1 = accounts[1].clone();
    store
        .update_balances_for_prepare(accounts[0].clone(), 100)
        .await
        .unwrap();
    let balance0 = store.get_balance(accounts[0].clone()).await.unwrap();
    let balance1 = store.get_balance(accounts[1].clone()).await.unwrap();
    assert_eq!(balance0, -100);
    assert_eq!(balance1, 0);
    store
        .update_balances_for_reject(account0.clone(), 100)
        .await
        .unwrap();
    let balance0 = store.get_balance(accounts[0].clone()).await.unwrap();
    let balance1 = store.get_balance(accounts[1].clone()).await.unwrap();
    assert_eq!(balance0, 0);
    assert_eq!(balance1, 0);
}

#[tokio::test]
async fn enforces_minimum_balance() {
    let (store, _context, accs) = test_store().await.unwrap();
    let accounts = store
        .get_accounts(vec![accs[0].id(), accs[1].id()])
        .await
        .unwrap();
    let result = store
        .update_balances_for_prepare(accounts[0].clone(), 10000)
        .await;
    assert!(result.is_err());
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
    let accounts = store
        .get_accounts(vec![accs[0].id(), acc.id()])
        .await
        .unwrap();
    let account0 = accounts[0].clone();
    let account1 = accounts[1].clone();

    // decrement account 0 by 100
    store
        .update_balances_for_prepare(account0.clone(), 100)
        .await
        .unwrap();
    // increment account 1 by 100
    store
        .update_balances_for_fulfill(account1.clone(), 100)
        .await
        .unwrap();

    // decrement account 1 by 80
    store
        .update_balances_for_prepare(account1.clone(), 80)
        .await
        .unwrap();
    // increment account 0 by 80
    store
        .update_balances_for_fulfill(account0.clone(), 80)
        .await
        .unwrap();

    let balance0 = store.get_balance(accounts[0].clone()).await.unwrap();
    let balance1 = store.get_balance(accounts[1].clone()).await.unwrap();
    assert_eq!(balance0, -20);
    assert_eq!(balance1, 20);
}
