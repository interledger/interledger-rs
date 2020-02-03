use super::fixtures::*;
use super::store_helpers::*;

use interledger_api::NodeStore;
use interledger_btp::BtpAccount;
use interledger_http::{HttpAccount, HttpStore};
use interledger_packet::Address;
use interledger_service::{Account, Username};
use secrecy::{ExposeSecret, SecretString};
use std::str::FromStr;

#[tokio::test]
async fn gets_account_from_http_bearer_token() {
    let (store, _context, _) = test_store().await.unwrap();
    let account = store
        .get_account_from_http_auth(&Username::from_str("alice").unwrap(), "incoming_auth_token")
        .await
        .unwrap();
    assert_eq!(
        *account.ilp_address(),
        Address::from_str("example.alice").unwrap()
    );
    assert_eq!(
        account.get_http_auth_token().unwrap().expose_secret(),
        "outgoing_auth_token",
    );
    assert_eq!(
        &account.get_ilp_over_btp_outgoing_token().unwrap(),
        b"btp_token",
    );
}

#[tokio::test]
async fn errors_on_wrong_http_token() {
    let (store, _context, _) = test_store().await.unwrap();
    // wrong password
    let err = store
        .get_account_from_http_auth(&Username::from_str("alice").unwrap(), "unknown_token")
        .await
        .unwrap_err();
    assert_eq!(
        err.to_string(),
        "account `alice` is not authorized for this action"
    );
}

#[tokio::test]
async fn errors_on_unknown_user() {
    let (store, _context, _) = test_store().await.unwrap();
    // wrong user
    let err = store
        .get_account_from_http_auth(&Username::from_str("asdf").unwrap(), "incoming_auth_token")
        .await
        .unwrap_err();
    assert_eq!(err.to_string(), "account `asdf` was not found");
}

#[tokio::test]
async fn duplicate_http_incoming_auth_works() {
    let mut duplicate = ACCOUNT_DETAILS_2.clone();
    duplicate.ilp_over_http_incoming_token =
        Some(SecretString::new("incoming_auth_token".to_string()));
    let (store, _context, accs) = test_store().await.unwrap();
    let original = accs[0].clone();
    let original_id = original.id();
    let duplicate = store.insert_account(duplicate).await.unwrap();
    let duplicate_id = duplicate.id();
    assert_ne!(original_id, duplicate_id);
    let result = futures::future::join_all(vec![
        store.get_account_from_http_auth(
            &Username::from_str("alice").unwrap(),
            "incoming_auth_token",
        ),
        store.get_account_from_http_auth(
            &Username::from_str("charlie").unwrap(),
            "incoming_auth_token",
        ),
    ])
    .await;
    let accs: Vec<_> = result.into_iter().map(|r| r.unwrap()).collect();
    // Alice and Charlie had the same auth token, but they had a
    // different username/account id, so no problem.
    assert_ne!(accs[0].id(), accs[1].id());
    assert_eq!(accs[0].id(), original_id);
    assert_eq!(accs[1].id(), duplicate_id);
}
