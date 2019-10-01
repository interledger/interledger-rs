mod common;

use bytes::Bytes;
use common::*;
use futures::future::join_all;
use http::StatusCode;
use interledger_api::NodeStore;
use interledger_service::{Account, AccountStore};
use interledger_settlement::{IdempotentStore, LeftoversStore, SettlementAccount, SettlementStore};
use interledger_store_redis::AccountId;
use lazy_static::lazy_static;
use num_bigint::BigUint;
use redis::{aio::SharedConnection, cmd};
use url::Url;

lazy_static! {
    static ref IDEMPOTENCY_KEY: String = String::from("AJKJNUjM0oyiAN46");
}

#[test]
fn saves_and_gets_uncredited_settlement_amount_properly() {
    block_on(test_store().and_then(|(store, context, _accs)| {
        let amounts = vec![
            (BigUint::from(5u32), 11),   // 5
            (BigUint::from(855u32), 12), // 905
            (BigUint::from(1u32), 10),   // 1005 total
        ];
        let acc = AccountId::new();
        let mut f = Vec::new();
        for a in amounts {
            let s = store.clone();
            f.push(s.save_uncredited_settlement_amount(acc, a));
        }
        join_all(f)
            .map_err(|err| eprintln!("Redis error: {:?}", err))
            .and_then(move |_| {
                store
                    .load_uncredited_settlement_amount(acc, 9)
                    .map_err(|err| eprintln!("Redis error: {:?}", err))
                    .and_then(move |ret| {
                        // 1 uncredited unit for scale 9
                        assert_eq!(ret, BigUint::from(1u32));
                        // rest should be in the leftovers store
                        store
                            .get_uncredited_settlement_amount(acc)
                            .map_err(|err| eprintln!("Redis error: {:?}", err))
                            .and_then(move |ret| {
                                // 1 uncredited unit for scale 9
                                assert_eq!(ret, (BigUint::from(5u32), 12));
                                let _ = context;
                                Ok(())
                            })
                    })
            })
    }))
    .unwrap()
}

#[test]
fn credits_prepaid_amount() {
    block_on(test_store().and_then(|(store, context, accs)| {
        let id = accs[0].id();
        context.async_connection().and_then(move |conn| {
            store
                .update_balance_for_incoming_settlement(id, 100, Some(IDEMPOTENCY_KEY.clone()))
                .and_then(move |_| {
                    cmd("HMGET")
                        .arg(format!("accounts:{}", id))
                        .arg("balance")
                        .arg("prepaid_amount")
                        .query_async(conn)
                        .map_err(|err| eprintln!("Redis error: {:?}", err))
                        .and_then(move |(_conn, (balance, prepaid_amount)): (_, (i64, i64))| {
                            assert_eq!(balance, 0);
                            assert_eq!(prepaid_amount, 100);
                            let _ = context;
                            Ok(())
                        })
                })
        })
    }))
    .unwrap()
}

#[test]
fn saves_and_loads_idempotency_key_data_properly() {
    block_on(test_store().and_then(|(store, context, _accs)| {
        let input_hash: [u8; 32] = Default::default();
        store
            .save_idempotent_data(
                IDEMPOTENCY_KEY.clone(),
                input_hash,
                StatusCode::OK,
                Bytes::from("TEST"),
            )
            .map_err(|err| eprintln!("Redis error: {:?}", err))
            .and_then(move |_| {
                store
                    .load_idempotent_data(IDEMPOTENCY_KEY.clone())
                    .map_err(|err| eprintln!("Redis error: {:?}", err))
                    .and_then(move |data1| {
                        assert_eq!(
                            data1.unwrap(),
                            (StatusCode::OK, Bytes::from("TEST"), input_hash)
                        );
                        let _ = context;

                        store
                            .load_idempotent_data("asdf".to_string())
                            .map_err(|err| eprintln!("Redis error: {:?}", err))
                            .and_then(move |data2| {
                                assert!(data2.is_none());
                                let _ = context;
                                Ok(())
                            })
                    })
            })
    }))
    .unwrap();
}

#[test]
fn idempotent_settlement_calls() {
    block_on(test_store().and_then(|(store, context, accs)| {
        let id = accs[0].id();
        context.async_connection().and_then(move |conn| {
            store
                .update_balance_for_incoming_settlement(id, 100, Some(IDEMPOTENCY_KEY.clone()))
                .and_then(move |_| {
                    cmd("HMGET")
                        .arg(format!("accounts:{}", id))
                        .arg("balance")
                        .arg("prepaid_amount")
                        .query_async(conn)
                        .map_err(|err| eprintln!("Redis error: {:?}", err))
                        .and_then(move |(conn, (balance, prepaid_amount)): (_, (i64, i64))| {
                            assert_eq!(balance, 0);
                            assert_eq!(prepaid_amount, 100);

                            store
                                .update_balance_for_incoming_settlement(
                                    id,
                                    100,
                                    Some(IDEMPOTENCY_KEY.clone()), // Reuse key to make idempotent request.
                                )
                                .and_then(move |_| {
                                    cmd("HMGET")
                                        .arg(format!("accounts:{}", id))
                                        .arg("balance")
                                        .arg("prepaid_amount")
                                        .query_async(conn)
                                        .map_err(|err| eprintln!("Redis error: {:?}", err))
                                        .and_then(
                                            move |(_conn, (balance, prepaid_amount)): (
                                                _,
                                                (i64, i64),
                                            )| {
                                                // Since it's idempotent there
                                                // will be no state update.
                                                // Otherwise it'd be 200 (100 + 100)
                                                assert_eq!(balance, 0);
                                                assert_eq!(prepaid_amount, 100);
                                                let _ = context;
                                                Ok(())
                                            },
                                        )
                                })
                        })
                })
        })
    }))
    .unwrap()
}

#[test]
fn credits_balance_owed() {
    block_on(test_store().and_then(|(store, context, accs)| {
        let id = accs[0].id();
        context
            .shared_async_connection()
            .map_err(|err| panic!(err))
            .and_then(move |conn| {
                cmd("HSET")
                    .arg(format!("accounts:{}", id))
                    .arg("balance")
                    .arg(-200)
                    .query_async(conn)
                    .map_err(|err| panic!(err))
                    .and_then(move |(conn, _balance): (SharedConnection, i64)| {
                        store
                            .update_balance_for_incoming_settlement(
                                id,
                                100,
                                Some(IDEMPOTENCY_KEY.clone()),
                            )
                            .and_then(move |_| {
                                cmd("HMGET")
                                    .arg(format!("accounts:{}", id))
                                    .arg("balance")
                                    .arg("prepaid_amount")
                                    .query_async(conn)
                                    .map_err(|err| panic!(err))
                                    .and_then(
                                        move |(_conn, (balance, prepaid_amount)): (
                                            _,
                                            (i64, i64),
                                        )| {
                                            assert_eq!(balance, -100);
                                            assert_eq!(prepaid_amount, 0);
                                            let _ = context;
                                            Ok(())
                                        },
                                    )
                            })
                    })
            })
    }))
    .unwrap()
}

#[test]
fn clears_balance_owed() {
    block_on(test_store().and_then(|(store, context, accs)| {
        let id = accs[0].id();
        context
            .shared_async_connection()
            .map_err(|err| panic!(err))
            .and_then(move |conn| {
                cmd("HSET")
                    .arg(format!("accounts:{}", id))
                    .arg("balance")
                    .arg(-100)
                    .query_async(conn)
                    .map_err(|err| panic!(err))
                    .and_then(move |(conn, _balance): (SharedConnection, i64)| {
                        store
                            .update_balance_for_incoming_settlement(
                                id,
                                100,
                                Some(IDEMPOTENCY_KEY.clone()),
                            )
                            .and_then(move |_| {
                                cmd("HMGET")
                                    .arg(format!("accounts:{}", id))
                                    .arg("balance")
                                    .arg("prepaid_amount")
                                    .query_async(conn)
                                    .map_err(|err| panic!(err))
                                    .and_then(
                                        move |(_conn, (balance, prepaid_amount)): (
                                            _,
                                            (i64, i64),
                                        )| {
                                            assert_eq!(balance, 0);
                                            assert_eq!(prepaid_amount, 0);
                                            let _ = context;
                                            Ok(())
                                        },
                                    )
                            })
                    })
            })
    }))
    .unwrap()
}

#[test]
fn clears_balance_owed_and_puts_remainder_as_prepaid() {
    block_on(test_store().and_then(|(store, context, accs)| {
        let id = accs[0].id();
        context
            .shared_async_connection()
            .map_err(|err| panic!(err))
            .and_then(move |conn| {
                cmd("HSET")
                    .arg(format!("accounts:{}", id))
                    .arg("balance")
                    .arg(-40)
                    .query_async(conn)
                    .map_err(|err| panic!(err))
                    .and_then(move |(conn, _balance): (SharedConnection, i64)| {
                        store
                            .update_balance_for_incoming_settlement(
                                id,
                                100,
                                Some(IDEMPOTENCY_KEY.clone()),
                            )
                            .and_then(move |_| {
                                cmd("HMGET")
                                    .arg(format!("accounts:{}", id))
                                    .arg("balance")
                                    .arg("prepaid_amount")
                                    .query_async(conn)
                                    .map_err(|err| panic!(err))
                                    .and_then(
                                        move |(_conn, (balance, prepaid_amount)): (
                                            _,
                                            (i64, i64),
                                        )| {
                                            assert_eq!(balance, 0);
                                            assert_eq!(prepaid_amount, 60);
                                            let _ = context;
                                            Ok(())
                                        },
                                    )
                            })
                    })
            })
    }))
    .unwrap()
}

#[test]
fn loads_globally_configured_settlement_engine_url() {
    block_on(test_store().and_then(|(store, context, accs)| {
        assert!(accs[0].settlement_engine_details().is_some());
        assert!(accs[1].settlement_engine_details().is_none());
        let account_ids = vec![accs[0].id(), accs[1].id()];
        store
            .clone()
            .get_accounts(account_ids.clone())
            .and_then(move |accounts| {
                assert!(accounts[0].settlement_engine_details().is_some());
                assert!(accounts[1].settlement_engine_details().is_none());

                store
                    .clone()
                    .set_settlement_engines(vec![
                        (
                            "ABC".to_string(),
                            Url::parse("http://settle-abc.example").unwrap(),
                        ),
                        (
                            "XYZ".to_string(),
                            Url::parse("http://settle-xyz.example").unwrap(),
                        ),
                    ])
                    .and_then(move |_| {
                        store.get_accounts(account_ids).and_then(move |accounts| {
                            // It should not overwrite the one that was individually configured
                            assert_eq!(
                                accounts[0]
                                    .settlement_engine_details()
                                    .unwrap()
                                    .url
                                    .as_str(),
                                "http://settlement.example/"
                            );

                            // It should set the URL for the account that did not have one configured
                            assert!(accounts[1].settlement_engine_details().is_some());
                            assert_eq!(
                                accounts[1]
                                    .settlement_engine_details()
                                    .unwrap()
                                    .url
                                    .as_str(),
                                "http://settle-abc.example/"
                            );
                            let _ = context;
                            Ok(())
                        })
                    })
                // store.set_settlement_engines
            })
    }))
    .unwrap()
}
