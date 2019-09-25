mod common;

use common::*;
use futures::future::{self, Either};
use interledger_api::NodeStore;
use interledger_packet::Address;
use interledger_service::{AccountStore, Username};
use interledger_service_util::BalanceStore;
use std::str::FromStr;

use interledger_service::{Account as AccountTrait, AddressStore};
use interledger_store_redis::AccountId;

#[test]
fn get_balance() {
    block_on(test_store().and_then(|(store, context, _accs)| {
        let account_id = AccountId::new();
        context
            .async_connection()
            .map_err(move |err| panic!(err))
            .and_then(move |connection| {
                redis::cmd("HMSET")
                    .arg(format!("accounts:{}", account_id))
                    .arg("balance")
                    .arg(600)
                    .arg("prepaid_amount")
                    .arg(400)
                    .query_async(connection)
                    .map_err(|err| panic!(err))
                    .and_then(move |(_, _): (_, redis::Value)| {
                        let account = Account::try_from(
                            account_id,
                            ACCOUNT_DETAILS_0.clone(),
                            store.get_ilp_address(),
                        )
                        .unwrap();
                        store.get_balance(account).and_then(move |balance| {
                            assert_eq!(balance, 1000);
                            let _ = context;
                            Ok(())
                        })
                    })
            })
    }))
    .unwrap();
}

#[test]
fn prepare_then_fulfill_with_settlement() {
    block_on(test_store().and_then(|(store, context, accs)| {
        let store_clone_1 = store.clone();
        let store_clone_2 = store.clone();
        store
            .clone()
            .get_accounts(vec![accs[0].id(), accs[1].id()])
            .map_err(|_err| panic!("Unable to get accounts"))
            .and_then(move |accounts| {
                let account0 = accounts[0].clone();
                let account1 = accounts[1].clone();
                store
                    // reduce account 0's balance by 100
                    .update_balances_for_prepare(accounts[0].clone(), 100)
                    .and_then(move |_| {
                        store_clone_1
                            .clone()
                            .get_balance(accounts[0].clone())
                            .join(store_clone_1.clone().get_balance(accounts[1].clone()))
                            .and_then(|(balance0, balance1)| {
                                assert_eq!(balance0, -100);
                                assert_eq!(balance1, 0);
                                Ok(())
                            })
                    })
                    .and_then(move |_| {
                        store_clone_2
                            .clone()
                            .update_balances_for_fulfill(account1.clone(), 100)
                            .and_then(move |_| {
                                store_clone_2
                                    .clone()
                                    .get_balance(account0.clone())
                                    .join(store_clone_2.clone().get_balance(account1.clone()))
                                    .and_then(move |(balance0, balance1)| {
                                        assert_eq!(balance0, -100);
                                        assert_eq!(balance1, -1000); // the account must be settled down to -1000
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
fn process_fulfill_no_settle_to() {
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
    block_on(test_store().and_then(|(store, context, _accs)| {
        let store_clone = store.clone();
        store.clone().insert_account(acc).and_then(move |account| {
            let id = account.id();
            store_clone
                .get_accounts(vec![id])
                .and_then(move |accounts| {
                    let acc = accounts[0].clone();
                    store_clone
                        .clone()
                        .update_balances_for_fulfill(acc.clone(), 100)
                        .and_then(move |(balance, amount_to_settle)| {
                            assert_eq!(balance, 100);
                            assert_eq!(amount_to_settle, 0);
                            let _ = context;
                            Ok(())
                        })
                })
        })
    }))
    .unwrap();
}

#[test]
fn process_fulfill_settle_to_over_threshold() {
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
    block_on(test_store().and_then(|(store, context, _accs)| {
        let store_clone = store.clone();
        store.clone().insert_account(acc).and_then(move |acc| {
            let id = acc.id();
            store_clone
                .get_accounts(vec![id])
                .and_then(move |accounts| {
                    let acc = accounts[0].clone();
                    store_clone
                        .clone()
                        .update_balances_for_fulfill(acc.clone(), 1000)
                        .and_then(move |(balance, amount_to_settle)| {
                            assert_eq!(balance, 1000);
                            assert_eq!(amount_to_settle, 0);
                            let _ = context;
                            Ok(())
                        })
                })
        })
    }))
    .unwrap();
}

#[test]
fn process_fulfill_ok() {
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
    block_on(test_store().and_then(|(store, context, _accs)| {
        let store_clone = store.clone();
        store.clone().insert_account(acc).and_then(move |account| {
            let id = account.id();
            store_clone
                .get_accounts(vec![id])
                .and_then(move |accounts| {
                    let acc = accounts[0].clone();
                    store_clone
                        .clone()
                        .update_balances_for_fulfill(acc.clone(), 101)
                        .and_then(move |(balance, amount_to_settle)| {
                            assert_eq!(balance, 0);
                            assert_eq!(amount_to_settle, 101);
                            let _ = context;
                            Ok(())
                        })
                })
        })
    }))
    .unwrap();
}

#[test]
fn prepare_then_reject() {
    block_on(test_store().and_then(|(store, context, accs)| {
        let store_clone_1 = store.clone();
        let store_clone_2 = store.clone();
        store
            .clone()
            .get_accounts(vec![accs[0].id(), accs[1].id()])
            .map_err(|_err| panic!("Unable to get accounts"))
            .and_then(move |accounts| {
                let account0 = accounts[0].clone();
                let account1 = accounts[1].clone();
                store
                    .update_balances_for_prepare(accounts[0].clone(), 100)
                    .and_then(move |_| {
                        store_clone_1
                            .clone()
                            .get_balance(accounts[0].clone())
                            .join(store_clone_1.clone().get_balance(accounts[1].clone()))
                            .and_then(|(balance0, balance1)| {
                                assert_eq!(balance0, -100);
                                assert_eq!(balance1, 0);
                                Ok(())
                            })
                    })
                    .and_then(move |_| {
                        store_clone_2
                            .clone()
                            .update_balances_for_reject(account0.clone(), 100)
                            .and_then(move |_| {
                                store_clone_2
                                    .clone()
                                    .get_balance(account0.clone())
                                    .join(store_clone_2.clone().get_balance(account1.clone()))
                                    .and_then(move |(balance0, balance1)| {
                                        assert_eq!(balance0, 0);
                                        assert_eq!(balance1, 0);
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
fn enforces_minimum_balance() {
    block_on(test_store().and_then(|(store, context, accs)| {
        store
            .clone()
            .get_accounts(vec![accs[0].id(), accs[1].id()])
            .map_err(|_err| panic!("Unable to get accounts"))
            .and_then(move |accounts| {
                store
                    .update_balances_for_prepare(accounts[0].clone(), 10000)
                    .then(move |result| {
                        assert!(result.is_err());
                        let _ = context;
                        Ok(())
                    })
            })
    }))
    .unwrap()
}

#[test]
// Prepare and Fulfill a packet for 100 units from Account 0 to Account 1
// Then, Prepare and Fulfill a packet for 80 units from Account 1 to Account 0
fn netting_fulfilled_balances() {
    block_on(test_store().and_then(|(store, context, accs)| {
        let store_clone1 = store.clone();
        let store_clone2 = store.clone();
        store
            .clone()
            .insert_account(ACCOUNT_DETAILS_2.clone())
            .and_then(move |acc| {
                store
                    .clone()
                    .get_accounts(vec![accs[0].id(), acc.id()])
                    .map_err(|_err| panic!("Unable to get accounts"))
                    .and_then(move |accounts| {
                        let account0 = accounts[0].clone();
                        let account1 = accounts[1].clone();
                        let account0_clone = account0.clone();
                        let account1_clone = account1.clone();
                        future::join_all(vec![
                            Either::A(store.clone().update_balances_for_prepare(
                                account0.clone(),
                                100, // decrement account 0 by 100
                            )),
                            Either::B(
                                store
                                    .clone()
                                    .update_balances_for_fulfill(
                                        account1.clone(), // increment account 1 by 100
                                        100,
                                    )
                                    .and_then(|_| Ok(())),
                            ),
                        ])
                        .and_then(move |_| {
                            future::join_all(vec![
                                Either::A(
                                    store_clone1
                                        .clone()
                                        .update_balances_for_prepare(account1.clone(), 80),
                                ),
                                Either::B(
                                    store_clone1
                                        .clone()
                                        .update_balances_for_fulfill(account0.clone(), 80)
                                        .and_then(|_| Ok(())),
                                ),
                            ])
                        })
                        .and_then(move |_| {
                            store_clone2
                                .clone()
                                .get_balance(account0_clone)
                                .join(store_clone2.get_balance(account1_clone))
                                .and_then(move |(balance0, balance1)| {
                                    assert_eq!(balance0, -20);
                                    assert_eq!(balance1, 20);
                                    let _ = context;
                                    Ok(())
                                })
                        })
                    })
            })
    }))
    .unwrap();
}
