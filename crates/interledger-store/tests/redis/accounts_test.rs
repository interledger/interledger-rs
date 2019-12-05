use super::{fixtures::*, redis_helpers::*, store_helpers::*};
use futures::future::{result, Either, Future};
use interledger_api::{AccountSettings, NodeStore};
use interledger_btp::{BtpAccount, BtpStore};
use interledger_ccp::{CcpRoutingAccount, RoutingRelation};
use interledger_http::{HttpAccount, HttpStore};
use interledger_packet::Address;
use interledger_service::Account as AccountTrait;
use interledger_service::{AccountStore, AddressStore, Username};
use interledger_service_util::BalanceStore;
use interledger_store::redis::RedisStoreBuilder;
use log::{debug, error};
use redis_crate::Client;
use secrecy::ExposeSecret;
use secrecy::SecretString;
use std::str::FromStr;
use uuid::Uuid;

#[test]
fn picks_up_parent_during_initialization() {
    let context = TestContext::new();
    block_on(
        result(Client::open(context.get_client_connection_info()))
            .map_err(|err| error!("Error creating Redis client: {:?}", err))
            .and_then(|client| {
                debug!("Connected to redis: {:?}", client);
                client
                    .get_shared_async_connection()
                    .map_err(|err| error!("Error connecting to Redis: {:?}", err))
            })
            .and_then(move |connection| {
                // we set a parent that was already configured via perhaps a
                // previous account insertion. that means that when we connect
                // to the store we will always get the configured parent (if
                // there was one))
                redis_crate::cmd("SET")
                    .arg("parent_node_account_address")
                    .arg("example.bob.node")
                    .query_async(connection)
                    .map_err(|err| panic!(err))
                    .and_then(move |(_, _): (_, redis_crate::Value)| {
                        RedisStoreBuilder::new(context.get_client_connection_info(), [0; 32])
                            .connect()
                            .and_then(move |store| {
                                // the store's ilp address is the store's
                                // username appended to the parent's address
                                assert_eq!(
                                    store.get_ilp_address(),
                                    Address::from_str("example.bob.node").unwrap()
                                );
                                let _ = context;
                                Ok(())
                            })
                    })
            }),
    )
    .unwrap();
}

#[test]
fn insert_accounts() {
    block_on(test_store().and_then(|(store, context, _accs)| {
        store
            .insert_account(ACCOUNT_DETAILS_2.clone())
            .and_then(move |account| {
                assert_eq!(
                    *account.ilp_address(),
                    Address::from_str("example.alice.user1.charlie").unwrap()
                );
                let _ = context;
                Ok(())
            })
    }))
    .unwrap();
}

#[test]
fn update_ilp_and_children_addresses() {
    block_on(test_store().and_then(|(store, context, accs)| {
        store
            // Add a NonRoutingAccount to make sure its address
            // gets updated as well
            .insert_account(ACCOUNT_DETAILS_2.clone())
            .and_then(move |acc2| {
                let mut accs = accs.clone();
                accs.push(acc2);
                accs.sort_by_key(|a| a.username().clone());
                let ilp_address = Address::from_str("test.parent.our_address").unwrap();
                store
                    .set_ilp_address(ilp_address.clone())
                    .and_then(move |_| {
                        let ret = store.get_ilp_address();
                        assert_eq!(ilp_address, ret);
                        store.get_all_accounts().and_then(move |accounts: Vec<_>| {
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
                            let _ = context;
                            Ok(())
                        })
                    })
            })
    }))
    .unwrap();
}

#[test]
fn only_one_parent_allowed() {
    let mut acc = ACCOUNT_DETAILS_2.clone();
    acc.routing_relation = Some("Parent".to_owned());
    acc.username = Username::from_str("another_name").unwrap();
    acc.ilp_address = Some(Address::from_str("example.another_name").unwrap());
    block_on(test_store().and_then(|(store, context, accs)| {
        store.insert_account(acc.clone()).then(move |res| {
            // This should fail
            assert!(res.is_err());
            futures::future::join_all(vec![
                Either::A(store.delete_account(accs[0].id()).and_then(|_| Ok(()))),
                // must also clear the ILP Address to indicate that we no longer
                // have a parent account configured
                Either::B(store.clear_ilp_address()),
            ])
            .and_then(move |_| {
                store.insert_account(acc).and_then(move |_| {
                    // the call was successful, so the parent was succesfully added
                    let _ = context;
                    Ok(())
                })
            })
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
                redis_crate::cmd("HMSET")
                    .arg(format!("accounts:{}", id))
                    .arg("balance")
                    .arg(600)
                    .arg("prepaid_amount")
                    .arg(400)
                    .query_async(connection)
                    .map_err(|err| panic!(err))
                    .and_then(move |(_, _): (_, redis_crate::Value)| {
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
fn modify_account_settings_settle_to_overflow() {
    block_on(test_store().and_then(|(store, context, accounts)| {
        let mut settings = AccountSettings::default();
        // Redis.rs cannot save a value larger than i64::MAX
        settings.settle_to = Some(std::i64::MAX as u64 + 1);
        let account = accounts[0].clone();
        let id = account.id();
        store
            .modify_account_settings(id, settings)
            .then(move |ret| {
                assert!(ret.is_err());
                let _ = context;
                Ok(())
            })
    }))
    .unwrap();
}

use std::default::Default;
#[test]
fn modify_account_settings_unchanged() {
    block_on(test_store().and_then(|(store, context, accounts)| {
        let settings = AccountSettings::default();
        let account = accounts[0].clone();

        let id = account.id();
        store
            .modify_account_settings(id, settings)
            .and_then(move |ret| {
                assert_eq!(
                    account.get_http_auth_token().unwrap().expose_secret(),
                    ret.get_http_auth_token().unwrap().expose_secret(),
                );
                assert_eq!(
                    account.get_ilp_over_btp_outgoing_token().unwrap(),
                    ret.get_ilp_over_btp_outgoing_token().unwrap()
                );
                // Cannot check other parameters since they are only pub(crate).
                let _ = context;
                Ok(())
            })
    }))
    .unwrap();
}

#[test]
fn modify_account_settings() {
    block_on(test_store().and_then(|(store, context, accounts)| {
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
        store
            .modify_account_settings(id, settings)
            .and_then(move |ret| {
                assert_eq!(
                    ret.get_http_auth_token().unwrap().expose_secret(),
                    "test_token",
                );
                assert_eq!(
                    ret.get_ilp_over_btp_outgoing_token().unwrap(),
                    &b"dylan:test"[..],
                );
                // Cannot check other parameters since they are only pub(crate).
                let _ = context;
                Ok(())
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
    duplicate.ilp_over_http_incoming_token =
        Some(SecretString::new("incoming_auth_token".to_string()));
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
    charlie.ilp_over_btp_incoming_token = Some(SecretString::new("btp_token".to_string()));
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
                assert_eq!(accounts[0].ilp_address(), acc.ilp_address());
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
        let account_ids: Vec<Uuid> = accs.iter().rev().map(|a| a.id()).collect::<_>();
        store_clone
            .get_accounts(account_ids)
            .and_then(move |accounts| {
                // note reverse order is intentional
                assert_eq!(accounts[0].ilp_address(), accs[1].ilp_address());
                assert_eq!(accounts[1].ilp_address(), accs[0].ilp_address());
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
                    account.get_http_auth_token().unwrap().expose_secret(),
                    acc.get_http_auth_token().unwrap().expose_secret(),
                );
                assert_eq!(
                    account.get_ilp_over_btp_outgoing_token().unwrap(),
                    acc.get_ilp_over_btp_outgoing_token().unwrap(),
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
            .get_accounts(vec![Uuid::new_v4(), Uuid::new_v4()])
            .then(move |result| {
                let _ = context;
                result
            })
    }));
    assert!(result.is_err());
}
