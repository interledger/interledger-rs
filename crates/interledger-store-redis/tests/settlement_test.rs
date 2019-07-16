#[macro_use]
extern crate lazy_static;

mod common;

use bytes::Bytes;
use common::*;
use ethereum_tx_sign::web3::types::{Address as EthAddress, H256, U256};
use http::StatusCode;
use interledger_settlement::{IdempotentStore, SettlementStore};
use interledger_settlement_engines::EthereumAddresses;
use interledger_settlement_engines::EthereumStore;
use redis::{cmd, r#async::SharedConnection};
use std::str::FromStr;

lazy_static! {
    static ref IDEMPOTENCY_KEY: String = String::from("AJKJNUjM0oyiAN46");
}

#[test]
fn credits_prepaid_amount() {
    block_on(test_store().and_then(|(store, context)| {
        context.async_connection().and_then(move |conn| {
            store
                .update_balance_for_incoming_settlement(0, 100, Some(IDEMPOTENCY_KEY.clone()))
                .and_then(move |_| {
                    cmd("HMGET")
                        .arg("accounts:0")
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
fn saves_and_loads_ethereum_addreses_properly() {
    block_on(test_store().and_then(|(store, context)| {
        let account_ids = vec![30, 42];
        let account_addresses = vec![
            EthereumAddresses {
                own_address: EthAddress::from_str("3cdb3d9e1b74692bb1e3bb5fc81938151ca64b02")
                    .unwrap(),
                token_address: Some(
                    EthAddress::from_str("c92be489639a9c61f517bd3b955840fa19bc9b7c").unwrap(),
                ),
            },
            EthereumAddresses {
                own_address: EthAddress::from_str("2fcd07047c209c46a767f8338cb0b14955826826")
                    .unwrap(),
                token_address: None,
            },
        ];
        store
            .save_account_addresses(account_ids.clone(), account_addresses.clone())
            .map_err(|err| eprintln!("Redis error: {:?}", err))
            .and_then(move |_| {
                store
                    .load_account_addresses(account_ids.clone())
                    .map_err(|err| eprintln!("Redis error: {:?}", err))
                    .and_then(move |data| {
                        assert_eq!(data[0], account_addresses[0]);
                        assert_eq!(data[1], account_addresses[1]);
                        let _ = context;
                        store
                            .load_account_id_from_address(account_addresses[0])
                            .map_err(|err| eprintln!("Redis error: {:?}", err))
                            .and_then(move |acc_id| {
                                assert_eq!(acc_id, account_ids[0]);
                                let _ = context;
                                store
                                    .load_account_id_from_address(account_addresses[1])
                                    .map_err(|err| eprintln!("Redis error: {:?}", err))
                                    .and_then(move |acc_id| {
                                        assert_eq!(acc_id, account_ids[1]);
                                        let _ = context;
                                        Ok(())
                                    })
                            })
                    })
            })
    }))
    .unwrap()
}

#[test]
fn saves_and_loads_last_observed_data_properly() {
    block_on(test_store().and_then(|(store, context)| {
        let block = U256::from(2);
        let balance = U256::from(123);
        store
            .save_recently_observed_data(block, balance)
            .map_err(|err| eprintln!("Redis error: {:?}", err))
            .and_then(move |_| {
                store
                    .load_recently_observed_data()
                    .map_err(|err| eprintln!("Redis error: {:?}", err))
                    .and_then(move |data| {
                        assert_eq!(data.0, block);
                        assert_eq!(data.1, balance);
                        let _ = context;
                        Ok(())
                    })
            })
    }))
    .unwrap()
}

#[test]
fn saves_tx_hashes_properly() {
    block_on(test_store().and_then(|(store, context)| {
        let tx_hash =
            H256::from("0xb28675771f555adf614f1401838b9fffb43bc285387679bcbd313a8dc5bdc00e");
        store
            .check_tx_credited(tx_hash)
            .map_err(|err| eprintln!("Redis error: {:?}", err))
            .and_then(move |_| {
                store
                    .check_tx_credited(tx_hash)
                    .map_err(|err| eprintln!("Redis error: {:?}", err))
                    .and_then(move |_| {
                        let _ = context;
                        Ok(())
                    })
            })
    }))
    .unwrap_err()
}

#[test]
fn saves_and_loads_idempotency_key_data_properly() {
    block_on(test_store().and_then(|(store, context)| {
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
    .unwrap()
}

#[test]
fn idempotent_settlement_calls() {
    block_on(test_store().and_then(|(store, context)| {
        context.async_connection().and_then(move |conn| {
            store
                .update_balance_for_incoming_settlement(0, 100, Some(IDEMPOTENCY_KEY.clone()))
                .and_then(move |_| {
                    cmd("HMGET")
                        .arg("accounts:0")
                        .arg("balance")
                        .arg("prepaid_amount")
                        .query_async(conn)
                        .map_err(|err| eprintln!("Redis error: {:?}", err))
                        .and_then(move |(conn, (balance, prepaid_amount)): (_, (i64, i64))| {
                            assert_eq!(balance, 0);
                            assert_eq!(prepaid_amount, 100);

                            store
                                .update_balance_for_incoming_settlement(
                                    0,
                                    100,
                                    Some(IDEMPOTENCY_KEY.clone()), // Reuse key to make idempotent request.
                                )
                                .and_then(move |_| {
                                    cmd("HMGET")
                                        .arg("accounts:0")
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
    block_on(test_store().and_then(|(store, context)| {
        context
            .shared_async_connection()
            .map_err(|err| panic!(err))
            .and_then(move |conn| {
                cmd("HSET")
                    .arg("accounts:0")
                    .arg("balance")
                    .arg(-200)
                    .query_async(conn)
                    .map_err(|err| panic!(err))
                    .and_then(move |(conn, _balance): (SharedConnection, i64)| {
                        store
                            .update_balance_for_incoming_settlement(
                                0,
                                100,
                                Some(IDEMPOTENCY_KEY.clone()),
                            )
                            .and_then(move |_| {
                                cmd("HMGET")
                                    .arg("accounts:0")
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
    block_on(test_store().and_then(|(store, context)| {
        context
            .shared_async_connection()
            .map_err(|err| panic!(err))
            .and_then(move |conn| {
                cmd("HSET")
                    .arg("accounts:0")
                    .arg("balance")
                    .arg(-100)
                    .query_async(conn)
                    .map_err(|err| panic!(err))
                    .and_then(move |(conn, _balance): (SharedConnection, i64)| {
                        store
                            .update_balance_for_incoming_settlement(
                                0,
                                100,
                                Some(IDEMPOTENCY_KEY.clone()),
                            )
                            .and_then(move |_| {
                                cmd("HMGET")
                                    .arg("accounts:0")
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
    block_on(test_store().and_then(|(store, context)| {
        context
            .shared_async_connection()
            .map_err(|err| panic!(err))
            .and_then(move |conn| {
                cmd("HSET")
                    .arg("accounts:0")
                    .arg("balance")
                    .arg(-40)
                    .query_async(conn)
                    .map_err(|err| panic!(err))
                    .and_then(move |(conn, _balance): (SharedConnection, i64)| {
                        store
                            .update_balance_for_incoming_settlement(
                                0,
                                100,
                                Some(IDEMPOTENCY_KEY.clone()),
                            )
                            .and_then(move |_| {
                                cmd("HMGET")
                                    .arg("accounts:0")
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
