use super::store_helpers::*;
use futures::future::Future;
use interledger_btp::BtpAccount;
use interledger_http::{HttpAccount, HttpStore};
use interledger_packet::Address;
use interledger_service::{Account, Username};
use secrecy::ExposeSecret;
use std::str::FromStr;

#[test]
fn gets_account_from_http_bearer_token() {
    block_on(test_store().and_then(|(store, context, _accs)| {
        store
            .get_account_from_http_auth(
                &Username::from_str("alice").unwrap(),
                "incoming_auth_token",
            )
            .and_then(move |account| {
                assert_eq!(
                    *account.ilp_address(),
                    Address::from_str("example.alice").unwrap()
                );
                // this account is in Dylan's connector
                assert_eq!(
                    account.get_http_auth_token().unwrap().expose_secret(),
                    "outgoing_auth_token",
                );
                assert_eq!(
                    &account.get_ilp_over_btp_outgoing_token().unwrap(),
                    b"btp_token",
                );
                let _ = context;
                Ok(())
            })
    }))
    .unwrap()
}

#[test]
fn decrypts_outgoing_tokens_http() {
    block_on(test_store().and_then(|(store, context, _accs)| {
        store
            .get_account_from_http_auth(
                &Username::from_str("alice").unwrap(),
                "incoming_auth_token",
            )
            .and_then(move |account| {
                assert_eq!(
                    account.get_http_auth_token().unwrap().expose_secret(),
                    "outgoing_auth_token",
                );
                assert_eq!(
                    &account.get_ilp_over_btp_outgoing_token().unwrap(),
                    b"btp_token",
                );
                let _ = context;
                Ok(())
            })
    }))
    .unwrap()
}

#[test]
fn errors_on_unknown_http_auth() {
    let result = block_on(test_store().and_then(|(store, context, _accs)| {
        store
            .get_account_from_http_auth(&Username::from_str("someuser").unwrap(), "unknown_token")
            .then(move |result| {
                let _ = context;
                result
            })
    }));
    assert!(result.is_err());
}
