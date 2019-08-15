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
use interledger_store_redis::AccountId;
use std::str::FromStr;

#[test]
fn insert_accounts() {
    block_on(test_store().and_then(|(store, context, _accs)| {
        store
            .insert_account(ACCOUNT_DETAILS_2.clone())
            .and_then(move |account| {
                assert_eq!(
                    *account.client_address(),
                    Address::from_str("example.charlie").unwrap()
                );
                let _ = context;
                Ok(())
            })
    }))
    .unwrap();
}

#[test]
fn delete_accounts() {
    block_on(test_store().and_then(|(store, context, _accs)| {
        store.get_all_accounts().and_then(move |accounts| {
            let id = accounts[0].id();
            store.remove_account(id.clone()).and_then(move |_| {
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
    block_on(test_store().and_then(|(store, context, accs)| {
        let account0 = accs[0].clone();
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
    let result = block_on(test_store().and_then(|(store, context, _accs)| {
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
    let result = block_on(test_store().and_then(|(store, context, _accs)| {
        store.insert_account(account).then(move |result| {
            let _ = context;
            result
        })
    }));
    assert!(result.is_err());
}

#[test]
fn get_all_accounts() {
    block_on(test_store().and_then(|(store, context, _accs)| {
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
    block_on(test_store().and_then(|(store, context, accs)| {
        let store_clone = store.clone();
        let acc = accs[0].clone();
        store_clone
            .get_accounts(vec![acc.id()])
            .and_then(move |accounts| {
                assert_eq!(accounts[0].client_address(), acc.client_address(),);
                let _ = context;
                Ok(())
            })
    }))
    .unwrap();
}

#[test]
fn gets_multiple() {
    block_on(test_store().and_then(|(store, context, accs)| {
        let store_clone = store.clone();
        let accs = vec![accs[1].clone(), accs[0].clone()];
        let account_ids: Vec<AccountId> = accs.iter().map(|a| a.id()).collect::<_>();
        store_clone
            .get_accounts(account_ids)
            .and_then(move |accounts| {
                // note reverse order is intentional
                assert_eq!(accounts[0].client_address(), accs[0].client_address(),);
                assert_eq!(accounts[1].client_address(), accs[1].client_address(),);
                let _ = context;
                Ok(())
            })
    }))
    .unwrap();
}

#[test]
fn decrypts_outgoing_tokens_acc() {
    block_on(test_store().and_then(|(store, context, accs)| {
        let acc = accs[0].clone();
        store
            .get_accounts(vec![acc.id()])
            .and_then(move |accounts| {
                let account = accounts[0].clone();
                assert_eq!(
                    account.get_http_auth_token().unwrap(),
                    acc.get_http_auth_token().unwrap(),
                );
                assert_eq!(
                    account.get_btp_token().unwrap(),
                    acc.get_btp_token().unwrap(),
                );
                let _ = context;
                Ok(())
            })
    }))
    .unwrap()
}

#[test]
fn errors_for_unknown_accounts() {
    let result = block_on(test_store().and_then(|(store, context, _accs)| {
        store
            .get_accounts(vec![AccountId::new(), AccountId::new()])
            .then(move |result| {
                let _ = context;
                result
            })
    }));
    assert!(result.is_err());
}
