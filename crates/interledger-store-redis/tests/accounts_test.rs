mod common;

use common::*;

use interledger_api::NodeStore;
use interledger_btp::{BtpAccount, BtpStore};
use interledger_http::{HttpAccount, HttpStore};
use interledger_ildcp::IldcpAccount;
use interledger_packet::Address;
use interledger_service::Account as AccountTrait;
use interledger_service::{AccountStore, Username};
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
            store.delete_account(id).and_then(move |_| {
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
fn update_accounts() {
    block_on(test_store().and_then(|(store, context, accounts)| {
        context
            .async_connection()
            .map_err(|err| panic!(err))
            .and_then(move |connection| {
                let id = accounts[0].id();
                redis::cmd("HMSET")
                    .arg(format!("accounts:{}", id))
                    .arg("balance")
                    .arg(600)
                    .arg("prepaid_amount")
                    .arg(400)
                    .query_async(connection)
                    .map_err(|err| panic!(err))
                    .and_then(move |(_, _): (_, redis::Value)| {
                        let mut new = ACCOUNT_DETAILS_0.clone();
                        new.asset_code = String::from("TUV");
                        store.update_account(id, new).and_then(move |account| {
                            assert_eq!(account.asset_code(), "TUV");
                            store.get_balance(account).and_then(move |balance| {
                                assert_eq!(balance, 1000);
                                let _ = context;
                                Ok(())
                            })
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
fn fetches_account_from_username() {
    block_on(test_store().and_then(|(store, context, accs)| {
        store
            .get_account_id_from_username(&Username::from_str("alice").unwrap())
            .and_then(move |account_id| {
                assert_eq!(account_id, accs[0].id());
                let _ = context;
                Ok(())
            })
    }))
    .unwrap();
}

#[test]
fn duplicate_http_incoming_auth_works() {
    let mut duplicate = ACCOUNT_DETAILS_2.clone();
    duplicate.http_incoming_token = Some("incoming_auth_token".to_string());
    block_on(test_store().and_then(|(store, context, accs)| {
        let original = accs[0].clone();
        let original_id = original.id();
        store.insert_account(duplicate).and_then(move |duplicate| {
            let duplicate_id = duplicate.id();
            assert_ne!(original_id, duplicate_id);
            futures::future::join_all(vec![
                store.get_account_from_http_auth(
                    &Username::from_str("alice").unwrap(),
                    "incoming_auth_token",
                ),
                store.get_account_from_http_auth(
                    &Username::from_str("charlie").unwrap(),
                    "incoming_auth_token",
                ),
            ])
            .and_then(move |accs| {
                // Alice and Charlie had the same auth token, but they had a
                // different username/account id, so no problem.
                assert_ne!(accs[0].id(), accs[1].id());
                assert_eq!(accs[0].id(), original_id);
                assert_eq!(accs[1].id(), duplicate_id);
                let _ = context;
                Ok(())
            })
        })
    }))
    .unwrap();
}

#[test]
fn gets_account_from_btp_auth() {
    block_on(test_store().and_then(|(store, context, accs)| {
        // alice's incoming btp token is the username/password to get her
        // account's information
        store
            .get_account_from_btp_auth(&Username::from_str("alice").unwrap(), "btp_token")
            .and_then(move |acc| {
                assert_eq!(acc.id(), accs[0].id());
                let _ = context;
                Ok(())
            })
    }))
    .unwrap();
}

#[test]
fn gets_account_from_http_auth() {
    block_on(test_store().and_then(|(store, context, accs)| {
        store
            .get_account_from_http_auth(
                &Username::from_str("alice").unwrap(),
                "incoming_auth_token",
            )
            .and_then(move |acc| {
                assert_eq!(acc.id(), accs[0].id());
                let _ = context;
                Ok(())
            })
    }))
    .unwrap();
}

#[test]
fn duplicate_btp_incoming_auth_works() {
    let mut charlie = ACCOUNT_DETAILS_2.clone();
    charlie.btp_incoming_token = Some("btp_token".to_string());
    block_on(test_store().and_then(|(store, context, accs)| {
        let alice = accs[0].clone();
        let alice_id = alice.id();
        store.insert_account(charlie).and_then(move |charlie| {
            let charlie_id = charlie.id();
            assert_ne!(alice_id, charlie_id);
            futures::future::join_all(vec![
                store.get_account_from_btp_auth(&Username::from_str("alice").unwrap(), "btp_token"),
                store.get_account_from_btp_auth(
                    &Username::from_str("charlie").unwrap(),
                    "btp_token",
                ),
            ])
            .and_then(move |accs| {
                assert_ne!(accs[0].id(), accs[1].id());
                assert_eq!(accs[0].id(), alice_id);
                assert_eq!(accs[1].id(), charlie_id);
                let _ = context;
                Ok(())
            })
        })
    }))
    .unwrap();
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
        // set account ids in reverse order
        let account_ids: Vec<AccountId> = accs.iter().rev().map(|a| a.id()).collect::<_>();
        store_clone
            .get_accounts(account_ids)
            .and_then(move |accounts| {
                // note reverse order is intentional
                assert_eq!(accounts[0].client_address(), accs[1].client_address());
                assert_eq!(accounts[1].client_address(), accs[0].client_address());
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
