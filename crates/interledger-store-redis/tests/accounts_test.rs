mod common;

use common::*;

use interledger_api::NodeStore;
use interledger_btp::BtpAccount;
use interledger_http::HttpAccount;
use interledger_ildcp::IldcpAccount;
use interledger_packet::Address;
use interledger_service::Account as AccontTrait;
use interledger_service::AccountStore;
use interledger_service_util::BalanceStore;
use std::str::FromStr;

#[test]
fn insert_accounts() {
    block_on(test_store().and_then(|(store, context)| {
        store
            .insert_account(ACCOUNT_DETAILS_2.clone())
            .and_then(move |account| {
                assert_eq!(account.id(), 2);
                let _ = context;
                Ok(())
            })
    }))
    .unwrap();
}

#[test]
fn delete_accounts() {
    block_on(test_store().and_then(|(store, context)| {
        store.get_all_accounts().and_then(move |accounts| {
            let id = accounts[0].id();
            store.remove_account(id).and_then(move |_| {
                store.get_all_accounts().and_then(move |accounts| {
                    for a in accounts {
                        assert_ne!(id, a.id());
                    }
                    let _ = context;
                    Ok(())
                })
            })
        })
    }))
    .unwrap();
}

#[test]
fn starts_with_zero_balance() {
    block_on(test_store().and_then(|(store, context)| {
        let account0 = Account::try_from(0, ACCOUNT_DETAILS_0.clone()).unwrap();
        store.get_balance(account0).and_then(move |balance| {
            assert_eq!(balance, 0);
            let _ = context;
            Ok(())
        })
    }))
    .unwrap();
}

#[test]
fn fails_on_duplicate_http_incoming_auth() {
    let mut account = ACCOUNT_DETAILS_2.clone();
    account.http_incoming_token = Some("incoming_auth_token".to_string());
    let result = block_on(test_store().and_then(|(store, context)| {
        store.insert_account(account).then(move |result| {
            let _ = context;
            result
        })
    }));
    assert!(result.is_err());
}

#[test]
fn fails_on_duplicate_btp_incoming_auth() {
    let mut account = ACCOUNT_DETAILS_2.clone();
    account.btp_incoming_token = Some("btp_token".to_string());
    let result = block_on(test_store().and_then(|(store, context)| {
        store.insert_account(account).then(move |result| {
            let _ = context;
            result
        })
    }));
    assert!(result.is_err());
}

#[test]
fn get_all_accounts() {
    block_on(test_store().and_then(|(store, context)| {
        store.get_all_accounts().and_then(move |accounts| {
            assert_eq!(accounts.len(), 2);
            let _ = context;
            Ok(())
        })
    }))
    .unwrap();
}

#[test]
fn gets_single_account() {
    block_on(test_store().and_then(|(store, context)| {
        store.get_accounts(vec![1]).and_then(move |accounts| {
            assert_eq!(
                accounts[0].client_address(),
                &Address::from_str("example.bob").unwrap()
            );
            let _ = context;
            Ok(())
        })
    }))
    .unwrap();
}

#[test]
fn gets_multiple() {
    block_on(test_store().and_then(|(store, context)| {
        store.get_accounts(vec![1, 0]).and_then(move |accounts| {
            // note reverse order is intentional
            assert_eq!(
                accounts[0].client_address(),
                &Address::from_str("example.bob").unwrap()
            );
            assert_eq!(
                accounts[1].client_address(),
                &Address::from_str("example.alice").unwrap()
            );
            let _ = context;
            Ok(())
        })
    }))
    .unwrap();
}

#[test]
fn decrypts_outgoing_tokens() {
    block_on(test_store().and_then(|(store, context)| {
        store.get_accounts(vec![0]).and_then(move |accounts| {
            let account = &accounts[0];
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
fn errors_for_unknown_accounts() {
    let result = block_on(test_store().and_then(|(store, context)| {
        store.get_accounts(vec![0, 5]).then(move |result| {
            let _ = context;
            result
        })
    }));
    assert!(result.is_err());
}
