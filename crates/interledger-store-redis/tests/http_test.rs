#[macro_use]
extern crate lazy_static;

mod common;

use common::*;
use interledger_btp::BtpAccount;
use interledger_http::{HttpAccount, HttpStore};
use interledger_service::Account as AccountTrait;

#[test]
fn gets_account_from_http_bearer_token() {
    block_on(test_store().and_then(|(store, context)| {
        store
            .get_account_from_http_token("incoming_auth_token")
            .and_then(move |account| {
                assert_eq!(account.id(), 0);
                assert_eq!(
                    account.get_http_auth_token().unwrap(),
                    "outgoing_auth_token"
                );
                assert_eq!(account.get_btp_token().unwrap(), b"btp_token");
                let _ = context;
                Ok(())
            })
    }))
    .unwrap()
}

#[test]
fn decrypts_outgoing_tokens() {
    block_on(test_store().and_then(|(store, context)| {
        store
            .get_account_from_http_token("incoming_auth_token")
            .and_then(move |account| {
                assert_eq!(
                    account.get_http_auth_token().unwrap(),
                    "outgoing_auth_token"
                );
                assert_eq!(account.get_btp_token().unwrap(), b"btp_token");
                let _ = context;
                Ok(())
            })
    }))
    .unwrap()
}

#[test]
fn errors_on_unknown_http_auth() {
    let result = block_on(test_store().and_then(|(store, context)| {
        store
            .get_account_from_http_token("unknown_token")
            .then(move |result| {
                let _ = context;
                result
            })
    }));
    assert!(result.is_err());
}
