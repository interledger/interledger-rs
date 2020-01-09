use super::{fixtures::*, redis_helpers::*, store_helpers::*};
use futures::future::{Either, Future};
use futures::TryFutureExt;
use interledger_api::{AccountSettings, NodeStore};
use interledger_btp::{BtpAccount, BtpStore};
use interledger_ccp::{CcpRoutingAccount, RoutingRelation};
use interledger_http::{HttpAccount, HttpStore};
use interledger_packet::Address;
use interledger_service::Account as AccountTrait;
use interledger_service::{AccountStore, AddressStore, Username};
use interledger_service_util::BalanceStore;
use interledger_store::redis::RedisStoreBuilder;
use redis_crate::Client;
use secrecy::ExposeSecret;
use secrecy::SecretString;
use std::default::Default;
use std::str::FromStr;
use uuid::Uuid;

#[tokio::test]
async fn picks_up_parent_during_initialization() {
    let context = TestContext::new();
    let client = Client::open(context.get_client_connection_info()).unwrap();
    let mut connection = client.get_multiplexed_tokio_connection().await.unwrap();

    // we set a parent that was already configured via perhaps a
    // previous account insertion. that means that when we connect
    // to the store we will always get the configured parent (if
    // there was one))
    let _: redis_crate::Value = redis_crate::cmd("SET")
        .arg("parent_node_account_address")
        .arg("example.bob.node")
        .query_async(&mut connection)
        .await
        .unwrap();

    let store = RedisStoreBuilder::new(context.get_client_connection_info(), [0; 32])
        .connect()
        .await
        .unwrap();
    // the store's ilp address is the store's
    // username appended to the parent's address
    assert_eq!(
        store.get_ilp_address(),
        Address::from_str("example.bob.node").unwrap()
    );
}

#[tokio::test]
async fn insert_accounts() {
    let (store, _context, _) = test_store().await.unwrap();
    let account = store
        .insert_account(ACCOUNT_DETAILS_2.clone())
        .await
        .unwrap();
    assert_eq!(
        *account.ilp_address(),
        Address::from_str("example.alice.user1.charlie").unwrap()
    );
}

#[tokio::test]
async fn update_ilp_and_children_addresses() {
    let (store, _context, accs) = test_store().await.unwrap();
    // Add a NonRoutingAccount to make sure its address
    // gets updated as well
    let acc2 = store
        .insert_account(ACCOUNT_DETAILS_2.clone())
        .await
        .unwrap();
    let mut accs = accs.clone();
    accs.push(acc2);
    accs.sort_by_key(|a| a.username().clone());
    let ilp_address = Address::from_str("test.parent.our_address").unwrap();

    store.set_ilp_address(ilp_address.clone()).await.unwrap();
    let ret = store.get_ilp_address();
    assert_eq!(ilp_address, ret);

    let accounts = store.get_all_accounts().await.unwrap();
    let mut accounts = accounts.clone();
    accounts.sort_by(|a, b| {
        a.username()
            .as_bytes()
            .partial_cmp(b.username().as_bytes())
            .unwrap()
    });
    for (a, b) in accounts.into_iter().zip(&accs) {
        if a.routing_relation() == RoutingRelation::Child
            || a.routing_relation() == RoutingRelation::NonRoutingAccount
        {
            assert_eq!(
                *a.ilp_address(),
                ilp_address.with_suffix(a.username().as_bytes()).unwrap()
            );
        } else {
            assert_eq!(a.ilp_address(), b.ilp_address());
        }
    }
}

#[tokio::test]
async fn only_one_parent_allowed() {
    let mut acc = ACCOUNT_DETAILS_2.clone();
    acc.routing_relation = Some("Parent".to_owned());
    acc.username = Username::from_str("another_name").unwrap();
    acc.ilp_address = Some(Address::from_str("example.another_name").unwrap());
    let (store, _context, accs) = test_store().await.unwrap();
    let res = store.insert_account(acc.clone()).await;
    // This should fail
    assert!(res.is_err());
    futures::future::join_all(vec![
        Either::Left(store.delete_account(accs[0].id()).map_ok(|_| ())),
        // must also clear the ILP Address to indicate that we no longer
        // have a parent account configured
        Either::Right(store.clear_ilp_address()),
    ])
    .await;
    let res = store.insert_account(acc).await;
    assert!(res.is_ok());
}

#[tokio::test]
async fn delete_accounts() {
    let (store, _context, _) = test_store().await.unwrap();
    let accounts = store.get_all_accounts().await.unwrap();
    let id = accounts[0].id();
    store.delete_account(id).await.unwrap();
    let accounts = store.get_all_accounts().await.unwrap();
    for a in accounts {
        assert_ne!(id, a.id());
    }
}

#[tokio::test]
async fn update_accounts() {
    let (store, context, accounts) = test_store().await.unwrap();
    let mut connection = context.async_connection().await.unwrap();
    let id = accounts[0].id();
    let _: redis_crate::Value = redis_crate::cmd("HMSET")
        .arg(format!("accounts:{}", id))
        .arg("balance")
        .arg(600u64)
        .arg("prepaid_amount")
        .arg(400u64)
        .query_async(&mut connection)
        .await
        .unwrap();
    let mut new = ACCOUNT_DETAILS_0.clone();
    new.asset_code = String::from("TUV");
    let account = store.update_account(id, new).await.unwrap();
    assert_eq!(account.asset_code(), "TUV");
    let balance = store.get_balance(account).await.unwrap();
    assert_eq!(balance, 1000);
}

#[tokio::test]
async fn modify_account_settings_settle_to_overflow() {
    let (store, _context, accounts) = test_store().await.unwrap();
    let mut settings = AccountSettings::default();
    // Redis.rs cannot save a value larger than i64::MAX
    settings.settle_to = Some(std::i64::MAX as u64 + 1);
    let account = accounts[0].clone();
    let id = account.id();
    let ret = store.modify_account_settings(id, settings).await;
    assert!(ret.is_err());
}

#[tokio::test]
async fn modify_account_settings_unchanged() {
    let (store, _context, accounts) = test_store().await.unwrap();
    let settings = AccountSettings::default();
    let account = accounts[0].clone();

    let id = account.id();
    let ret = store.modify_account_settings(id, settings).await.unwrap();

    assert_eq!(
        account.get_http_auth_token().unwrap().expose_secret(),
        ret.get_http_auth_token().unwrap().expose_secret(),
    );
    assert_eq!(
        account.get_ilp_over_btp_outgoing_token().unwrap(),
        ret.get_ilp_over_btp_outgoing_token().unwrap()
    );
}

#[tokio::test]
async fn modify_account_settings() {
    let (store, _context, accounts) = test_store().await.unwrap();
    let settings = AccountSettings {
        ilp_over_http_outgoing_token: Some(SecretString::new("test_token".to_owned())),
        ilp_over_http_incoming_token: Some(SecretString::new("http_in_new".to_owned())),
        ilp_over_btp_outgoing_token: Some(SecretString::new("dylan:test".to_owned())),
        ilp_over_btp_incoming_token: Some(SecretString::new("btp_in_new".to_owned())),
        ilp_over_http_url: Some("http://example.com/accounts/dylan/ilp".to_owned()),
        ilp_over_btp_url: Some("http://example.com/accounts/dylan/ilp/btp".to_owned()),
        settle_threshold: Some(-50),
        settle_to: Some(100),
    };
    let account = accounts[0].clone();

    let id = account.id();
    let ret = store.modify_account_settings(id, settings).await.unwrap();
    assert_eq!(
        ret.get_http_auth_token().unwrap().expose_secret(),
        "test_token",
    );
    assert_eq!(
        ret.get_ilp_over_btp_outgoing_token().unwrap(),
        &b"dylan:test"[..],
    );
}

#[tokio::test]
async fn starts_with_zero_balance() {
    let (store, _context, accs) = test_store().await.unwrap();
    let account0 = accs[0].clone();
    let balance = store.get_balance(account0).await.unwrap();
    assert_eq!(balance, 0);
}

#[tokio::test]
async fn fetches_account_from_username() {
    let (store, _context, accs) = test_store().await.unwrap();
    let account_id = store
        .get_account_id_from_username(&Username::from_str("alice").unwrap())
        .await
        .unwrap();
    assert_eq!(account_id, accs[0].id());
}

#[tokio::test]
async fn get_all_accounts() {
    let (store, _context, _) = test_store().await.unwrap();
    let accounts = store.get_all_accounts().await.unwrap();
    assert_eq!(accounts.len(), 2);
}

#[tokio::test]
async fn gets_single_account() {
    let (store, _context, accs) = test_store().await.unwrap();
    let acc = accs[0].clone();
    let accounts = store.get_accounts(vec![acc.id()]).await.unwrap();
    assert_eq!(accounts[0].ilp_address(), acc.ilp_address());
}

#[tokio::test]
async fn gets_multiple() {
    let (store, _context, accs) = test_store().await.unwrap();
    // set account ids in reverse order
    let account_ids: Vec<Uuid> = accs.iter().rev().map(|a| a.id()).collect::<_>();
    let accounts = store.get_accounts(account_ids).await.unwrap();
    // note reverse order is intentional
    assert_eq!(accounts[0].ilp_address(), accs[1].ilp_address());
    assert_eq!(accounts[1].ilp_address(), accs[0].ilp_address());
}

#[tokio::test]
async fn decrypts_outgoing_tokens_acc() {
    let (store, _context, accs) = test_store().await.unwrap();
    let acc = accs[0].clone();
    let accounts = store.get_accounts(vec![acc.id()]).await.unwrap();
    let account = accounts[0].clone();
    assert_eq!(
        account.get_http_auth_token().unwrap().expose_secret(),
        acc.get_http_auth_token().unwrap().expose_secret(),
    );
    assert_eq!(
        account.get_ilp_over_btp_outgoing_token().unwrap(),
        acc.get_ilp_over_btp_outgoing_token().unwrap(),
    );
}

#[tokio::test]
async fn errors_for_unknown_accounts() {
    let (store, _context, _) = test_store().await.unwrap();
    let result = store
        .get_accounts(vec![Uuid::new_v4(), Uuid::new_v4()])
        .await;
    assert!(result.is_err());
}
