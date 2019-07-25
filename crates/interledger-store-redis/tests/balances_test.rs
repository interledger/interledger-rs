mod common;

use common::*;
use futures::future;
use interledger_service::AccountStore;
use interledger_service_util::BalanceStore;

#[test]
fn get_balance() {
    block_on(test_store().and_then(|(store, context)| {
        context
            .async_connection()
            .map_err(|err| panic!(err))
            .and_then(|connection| {
                redis::cmd("HMSET")
                    .arg("accounts:0")
                    .arg("balance")
                    .arg(600)
                    .arg("prepaid_amount")
                    .arg(400)
                    .query_async(connection)
                    .map_err(|err| panic!(err))
                    .and_then(move |(_, _): (_, redis::Value)| {
                        let account = Account::try_from(0, ACCOUNT_DETAILS_0.clone()).unwrap();
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
    block_on(test_store().and_then(|(store, context)| {
        let store_clone_1 = store.clone();
        let store_clone_2 = store.clone();
        store
            .clone()
            .get_accounts(vec![0, 1])
            .map_err(|_err| panic!("Unable to get accounts"))
            .and_then(move |accounts| {
                let account0 = accounts[0].clone();
                let account1 = accounts[1].clone();
                store
                    // nothing happens with the outgoing amount for prepare
                    .update_balances_for_prepare(accounts[0].clone(), 100, accounts[1].clone(), 0)
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
                            // nothing happens with the incoming amount for fulfill
                            .update_balances_for_fulfill(account0.clone(), 0, account1.clone(), 100)
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
fn prepare_then_reject() {
    block_on(test_store().and_then(|(store, context)| {
        let store_clone_1 = store.clone();
        let store_clone_2 = store.clone();
        store
            .clone()
            .get_accounts(vec![0, 1])
            .map_err(|_err| panic!("Unable to get accounts"))
            .and_then(move |accounts| {
                let account0 = accounts[0].clone();
                let account1 = accounts[1].clone();
                store
                    .update_balances_for_prepare(accounts[0].clone(), 100, accounts[1].clone(), 500)
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
                            .update_balances_for_reject(
                                account0.clone(),
                                100,
                                account1.clone(),
                                500,
                            )
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
    block_on(test_store().and_then(|(store, context)| {
        store
            .clone()
            .get_accounts(vec![0, 1])
            .map_err(|_err| panic!("Unable to get accounts"))
            .and_then(move |accounts| {
                store
                    .update_balances_for_prepare(
                        accounts[0].clone(),
                        10000,
                        accounts[1].clone(),
                        500,
                    )
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
fn netting_fulfilled_balances() {
    block_on(test_other_store().and_then(|(store, context)| {
        let store_clone1 = store.clone();
        let store_clone2 = store.clone();
        store
            .clone()
            .get_accounts(vec![0, 1])
            .map_err(|_err| panic!("Unable to get accounts"))
            .and_then(move |accounts| {
                let account0 = accounts[0].clone();
                let account1 = accounts[1].clone();
                let account0_clone = account0.clone();
                let account1_clone = account1.clone();
                future::join_all(vec![
                    store.clone().update_balances_for_prepare(
                        account0.clone(),
                        100,              // decrement account0 by 100
                        account1.clone(), // unused
                        0,                // outgoing amount is not used in prepare
                    ),
                    store.clone().update_balances_for_fulfill(
                        account0.clone(), // unused
                        0,                // incoming amount is not used for prepare
                        account1.clone(), // increment account 0 by 100
                        100,
                    ), //
                ])
                .and_then(move |_| {
                    future::join_all(vec![
                        store_clone1.clone().update_balances_for_prepare(
                            account1.clone(),
                            80,
                            account0.clone(),
                            0, // outgoing amount is not used in prepare
                        ),
                        store_clone1.clone().update_balances_for_fulfill(
                            account1.clone(),
                            0, // incoming amount is not used for prepare
                            account0.clone(),
                            80,
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
    }))
    .unwrap();
}
