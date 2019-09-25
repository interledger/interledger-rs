mod common;

use common::*;
use interledger_btp::{BtpAccount, BtpStore};
use interledger_http::HttpAccount;
use interledger_packet::Address;
use interledger_service::{Account, Username};
use std::str::FromStr;

#[test]
fn gets_account_from_btp_auth() {
    block_on(test_store().and_then(|(store, context, _accs)| {
        store
            .get_account_from_btp_auth(&Username::from_str("bob").unwrap(), "other_btp_token")
            .and_then(move |account| {
                assert_eq!(
                    *account.ilp_address(),
                    Address::from_str("example.alice.user1.bob").unwrap()
                );
                let _ = context;
                Ok(())
            })
    }))
    .unwrap()
}

#[test]
fn decrypts_outgoing_tokens_btp() {
    block_on(test_store().and_then(|(store, context, _accs)| {
        store
            .get_account_from_btp_auth(&Username::from_str("bob").unwrap(), "other_btp_token")
            .and_then(move |account| {
                // the account is created on Dylan's connector
                assert_eq!(
                    account.get_http_auth_token().unwrap(),
                    &format!("{}:outgoing_auth_token", "dylan"),
                );
                assert_eq!(
                    &account.get_ilp_over_btp_outgoing_token().unwrap(),
                    &format!("{}:btp_token", "dylan").as_bytes(),
                );
                let _ = context;
                Ok(())
            })
    }))
    .unwrap()
}

#[test]
fn errors_on_unknown_btp_token() {
    let result = block_on(test_store().and_then(|(store, context, _accs)| {
        store
            .get_account_from_btp_auth(
                &Username::from_str("someuser").unwrap(),
                "unknown_btp_token",
            )
            .then(move |result| {
                let _ = context;
                result
            })
    }));
    assert!(result.is_err());
}
