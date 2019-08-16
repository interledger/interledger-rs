mod common;

use bytes::Bytes;
use common::*;
use http::StatusCode;
use interledger_service::Account;
use interledger_settlement::{IdempotentStore, SettlementStore};
use lazy_static::lazy_static;
use redis::{cmd, r#async::SharedConnection};

lazy_static! {
    static ref IDEMPOTENCY_KEY: String = String::from("AJKJNUjM0oyiAN46");
}

#[test]
fn credits_prepaid_amount() {
    block_on(test_store().and_then(|(store, context, accs)| {
        let id = accs[0].id();
        context.async_connection().and_then(move |conn| {
            store
                .update_balance_for_incoming_settlement(
                    id.clone(),
                    100,
                    Some(IDEMPOTENCY_KEY.clone()),
                )
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
                .update_balance_for_incoming_settlement(
                    id.clone(),
                    100,
                    Some(IDEMPOTENCY_KEY.clone()),
                )
                .and_then(move |_| {
                    cmd("HMGET")
                        .arg(format!("accounts:{}", id.clone()))
                        .arg("balance")
                        .arg("prepaid_amount")
                        .query_async(conn)
                        .map_err(|err| eprintln!("Redis error: {:?}", err))
                        .and_then(move |(conn, (balance, prepaid_amount)): (_, (i64, i64))| {
                            assert_eq!(balance, 0);
                            assert_eq!(prepaid_amount, 100);

                            store
                                .update_balance_for_incoming_settlement(
                                    id.clone(),
                                    100,
                                    Some(IDEMPOTENCY_KEY.clone()), // Reuse key to make idempotent request.
                                )
                                .and_then(move |_| {
                                    cmd("HMGET")
                                        .arg(format!("accounts:{}", id.clone()))
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
                    .arg(format!("accounts:{}", id.clone()))
                    .arg("balance")
                    .arg(-200)
                    .query_async(conn)
                    .map_err(|err| panic!(err))
                    .and_then(move |(conn, _balance): (SharedConnection, i64)| {
                        store
                            .update_balance_for_incoming_settlement(
                                id.clone(),
                                100,
                                Some(IDEMPOTENCY_KEY.clone()),
                            )
                            .and_then(move |_| {
                                cmd("HMGET")
                                    .arg(format!("accounts:{}", id.clone()))
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
                    .arg(format!("accounts:{}", id.clone()))
                    .arg("balance")
                    .arg(-100)
                    .query_async(conn)
                    .map_err(|err| panic!(err))
                    .and_then(move |(conn, _balance): (SharedConnection, i64)| {
                        store
                            .update_balance_for_incoming_settlement(
                                id.clone(),
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
