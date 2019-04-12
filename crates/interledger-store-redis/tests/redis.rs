extern crate interledger_store_redis;
#[macro_use]
extern crate lazy_static;

use bytes::Bytes;
use env_logger;
use futures::{future, Future};
use interledger_api::{AccountDetails, NodeStore};
use interledger_store_redis::{connect, connect_with_poll_interval, Account, RedisStore};
use parking_lot::Mutex;
use redis;
use std::{
    collections::HashMap,
    time::{Duration, Instant},
};
use tokio::{runtime::Runtime, timer::Delay};

mod redis_helpers;
use redis_helpers::*;

lazy_static! {
    static ref ACCOUNT_DETAILS_0: AccountDetails = AccountDetails {
        ilp_address: "example.alice".to_string(),
        asset_scale: 6,
        asset_code: "XYZ".to_string(),
        max_packet_amount: 1000,
        min_balance: -1000,
        http_endpoint: Some("http://example.com/ilp".to_string()),
        http_incoming_token: Some("incoming_auth_token".to_string()),
        http_outgoing_token: Some("outgoing_auth_token".to_string()),
        btp_uri: Some("btp+ws://:btp_token@example.com/btp".to_string()),
        btp_incoming_token: Some("btp_token".to_string()),
        is_admin: true,
        xrp_address: Some("rELhRfZ7YS31jbouULKYLB64KmrizFuC3T".to_string()),
        settle_threshold: Some(0),
        settle_to: Some(-1000),
        send_routes: false,
        receive_routes: false,
        routing_relation: None,
        round_trip_time: None,
        amount_per_minute_limit: Some(1000),
        packets_per_minute_limit: Some(2),
    };
    static ref ACCOUNT_DETAILS_1: AccountDetails = AccountDetails {
        ilp_address: "example.bob".to_string(),
        asset_scale: 9,
        asset_code: "ABC".to_string(),
        max_packet_amount: 1_000_000,
        min_balance: 0,
        http_endpoint: Some("http://example.com/ilp".to_string()),
        http_incoming_token: Some("QWxhZGRpbjpPcGVuU2VzYW1l".to_string()),
        http_outgoing_token: Some("outgoing_auth_token".to_string()),
        btp_uri: Some("btp+ws://:other_outgoing_btp_token@example.com/btp".to_string()),
        btp_incoming_token: Some("other_btp_token".to_string()),
        is_admin: true,
        xrp_address: Some("rMLwdY4w8FT8zCEUL9q9173NrvpLGLEFDu".to_string()),
        settle_threshold: Some(0),
        settle_to: Some(-1000),
        send_routes: true,
        receive_routes: false,
        routing_relation: None,
        round_trip_time: None,
        amount_per_minute_limit: Some(1000),
        packets_per_minute_limit: Some(20),
    };
    static ref ACCOUNT_DETAILS_2: AccountDetails = AccountDetails {
        ilp_address: "example.charlie".to_string(),
        asset_scale: 9,
        asset_code: "XRP".to_string(),
        max_packet_amount: 1000,
        min_balance: 0,
        http_endpoint: None,
        http_incoming_token: None,
        http_outgoing_token: None,
        btp_uri: None,
        btp_incoming_token: None,
        is_admin: false,
        xrp_address: None,
        settle_threshold: Some(0),
        settle_to: Some(-1000),
        send_routes: false,
        receive_routes: false,
        routing_relation: None,
        round_trip_time: None,
        amount_per_minute_limit: None,
        packets_per_minute_limit: None,
    };
    static ref TEST_MUTEX: Mutex<()> = Mutex::new(());
}

fn test_store() -> impl Future<Item = (RedisStore, TestContext), Error = ()> {
    let context = TestContext::new();
    connect(context.get_client_connection_info(), [0; 32]).and_then(|store| {
        let store_clone = store.clone();
        store
            .clone()
            .insert_account(ACCOUNT_DETAILS_0.clone())
            .and_then(move |_| store_clone.insert_account(ACCOUNT_DETAILS_1.clone()))
            .and_then(|_| Ok((store, context)))
    })
}

fn block_on<F>(f: F) -> Result<F::Item, F::Error>
where
    F: Future + Send + 'static,
    F::Item: Send,
    F::Error: Send,
{
    // Only run one test at a time
    let _ = env_logger::try_init();
    let lock = TEST_MUTEX.lock();
    let mut runtime = Runtime::new().unwrap();
    let result = runtime.block_on(f);
    drop(lock);
    result
}

mod connect_store {
    use super::*;

    #[test]
    fn fails_if_db_unavailable() {
        let mut runtime = Runtime::new().unwrap();
        runtime
            .block_on(future::lazy(
                || -> Box<Future<Item = (), Error = ()> + Send> {
                    Box::new(connect("redis://127.0.0.1:0", [0; 32]).then(|result| {
                        assert!(result.is_err());
                        Ok(())
                    }))
                },
            ))
            .unwrap();
    }
}

mod insert_accounts {
    use super::*;
    use interledger_service::Account as AccontTrait;
    use interledger_service_util::BalanceStore;

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
    fn fails_on_duplicate_xrp_address() {
        let mut account = ACCOUNT_DETAILS_2.clone();
        account.xrp_address = Some("rELhRfZ7YS31jbouULKYLB64KmrizFuC3T".to_string());
        let result = block_on(test_store().and_then(|(store, context)| {
            store.insert_account(account).then(move |result| {
                let _ = context;
                result
            })
        }));
        assert!(result.is_err());
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

    fn credits_account_for_unclaimed_balance() {
        let xrp_address = "rJBKmQvMj4EMkGQA4dNV9hzQKnVJmKFfVa".to_string();
        let mut account = ACCOUNT_DETAILS_2.clone();
        account.xrp_address = Some(xrp_address.clone());
        block_on(test_store().and_then(|(store, context)| {
            context
                .async_connection()
                .map_err(|err| panic!(err))
                .and_then(|connection| {
                    redis::cmd("HSET")
                        .arg("unclaimed_balances:xrp")
                        .arg(xrp_address)
                        .arg(1_000_000)
                        .query_async(connection)
                        .map_err(|err| panic!(err))
                        .and_then(
                            move |(_connection, _val): (
                                redis::r#async::Connection,
                                redis::Value,
                            )| {
                                store
                                    .clone()
                                    .insert_account(account)
                                    .and_then(move |account| {
                                        store.get_balance(account).and_then(move |balance| {
                                            assert_eq!(balance, 1_000_000_000);
                                            let _ = context;
                                            Ok(())
                                        })
                                    })
                            },
                        )
                })
        }))
        .unwrap()
    }
}

mod node_store {
    use super::*;
    use interledger_api::NodeStore;
    use interledger_service_util::ExchangeRateStore;

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
    fn set_rates() {
        block_on(test_store().and_then(|(store, context)| {
            let store_clone = store.clone();
            let rates = store.get_exchange_rates(&["ABC", "XYZ"]);
            assert!(rates.is_err());
            store
                .set_rates(vec![("ABC".to_string(), 500.0), ("XYZ".to_string(), 0.005)])
                .and_then(move |_| {
                    let rates = store_clone.get_exchange_rates(&["XYZ", "ABC"]).unwrap();
                    assert_eq!(rates[0].to_string(), "0.005");
                    assert_eq!(rates[1].to_string(), "500");
                    let _ = context;
                    Ok(())
                })
        }))
        .unwrap();
    }
}

mod get_accounts {
    use super::*;
    use interledger_btp::BtpAccount;
    use interledger_http::HttpAccount;
    use interledger_ildcp::IldcpAccount;
    use interledger_service::AccountStore;

    #[test]
    fn gets_single_account() {
        block_on(test_store().and_then(|(store, context)| {
            store.get_accounts(vec![1]).and_then(move |accounts| {
                assert_eq!(accounts[0].client_address(), b"example.bob");
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
                assert_eq!(accounts[0].client_address(), b"example.bob");
                assert_eq!(accounts[1].client_address(), b"example.alice");
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
                assert_eq!(account.get_btp_token().unwrap().as_ref(), b"btp_token");
                let _ = context;
                Ok(())
            })
        }))
        .unwrap()
    }

    #[test]
    fn errors_for_unknown_accounts() {
        let result = block_on(test_store().and_then(|(store, context)| {
            store.get_accounts(vec![0, 2]).then(move |result| {
                let _ = context;
                result
            })
        }));
        assert!(result.is_err());
    }
}

mod routes_and_rates {
    use super::*;
    use interledger_router::RouterStore;
    use interledger_service_util::ExchangeRateStore;

    #[test]
    fn polls_for_route_updates() {
        let context = TestContext::new();
        block_on(
            connect_with_poll_interval(context.get_client_connection_info(), [0; 32], 1).and_then(
                |store| {
                    let connection = context.async_connection();
                    assert_eq!(store.routing_table().len(), 0);
                    let store_clone_1 = store.clone();
                    let store_clone_2 = store.clone();
                    store
                        .clone()
                        .insert_account(ACCOUNT_DETAILS_0.clone())
                        .and_then(move |_| {
                            let routing_table = store_clone_1.routing_table();
                            assert_eq!(routing_table.len(), 1);
                            assert_eq!(
                                *routing_table.get(&Bytes::from("example.alice")).unwrap(),
                                0
                            );
                            store_clone_1.insert_account(AccountDetails {
                                ilp_address: "example.bob".to_string(),
                                asset_scale: 6,
                                asset_code: "XYZ".to_string(),
                                max_packet_amount: 1000,
                                min_balance: -1000,
                                http_endpoint: None,
                                http_incoming_token: None,
                                http_outgoing_token: None,
                                btp_uri: None,
                                btp_incoming_token: None,
                                is_admin: false,
                                xrp_address: None,
                                settle_threshold: None,
                                settle_to: None,
                                send_routes: false,
                                receive_routes: false,
                                routing_relation: None,
                                round_trip_time: None,
                                amount_per_minute_limit: None,
                                packets_per_minute_limit: None,
                            })
                        })
                        .and_then(move |_| {
                            let routing_table = store_clone_2.routing_table();
                            assert_eq!(routing_table.len(), 2);
                            assert_eq!(*routing_table.get(&Bytes::from("example.bob")).unwrap(), 1);
                            connection
                                .map_err(|err| panic!(err))
                                .and_then(|connection| {
                                    redis::cmd("HMSET")
                                        .arg("routes")
                                        .arg("example.alice")
                                        .arg(1)
                                        .arg("example.charlie")
                                        .arg(0)
                                        .query_async(connection)
                                        .and_then(
                                            |(_connection, _result): (_, redis::Value)| Ok(()),
                                        )
                                        .map_err(|err| panic!(err))
                                        .and_then(|_| {
                                            Delay::new(Instant::now() + Duration::from_millis(10))
                                                .then(|_| Ok(()))
                                        })
                                })
                                .and_then(move |_| {
                                    let routing_table = store_clone_2.routing_table();
                                    assert_eq!(routing_table.len(), 3);
                                    assert_eq!(
                                        *routing_table.get(&Bytes::from("example.alice")).unwrap(),
                                        1
                                    );
                                    assert_eq!(
                                        *routing_table.get(&Bytes::from("example.bob")).unwrap(),
                                        1
                                    );
                                    assert_eq!(
                                        *routing_table
                                            .get(&Bytes::from("example.charlie"))
                                            .unwrap(),
                                        0
                                    );
                                    assert!(routing_table
                                        .get(&Bytes::from("example.other"))
                                        .is_none());
                                    let _ = context;
                                    Ok(())
                                })
                        })
                },
            ),
        )
        .unwrap();
    }

    #[test]
    fn polls_for_rate_updates() {
        let context = TestContext::new();
        block_on(
            connect_with_poll_interval(context.get_client_connection_info(), [0; 32], 1).and_then(
                |store| {
                    assert!(store.get_exchange_rates(&["ABC", "XYZ"]).is_err());
                    store
                        .clone()
                        .set_rates(vec![
                            ("ABC".to_string(), 0.5f64),
                            ("DEF".to_string(), 9_999_999_999.0f64),
                        ])
                        .and_then(|_| {
                            Delay::new(Instant::now() + Duration::from_millis(10)).then(|_| Ok(()))
                        })
                        .and_then(move |_| {
                            assert_eq!(store.get_exchange_rates(&["ABC"]).unwrap(), vec![0.5]);
                            assert_eq!(
                                store.get_exchange_rates(&["ABC", "DEF"]).unwrap(),
                                vec![0.5, 9_999_999_999.0]
                            );
                            assert!(store.get_exchange_rates(&["ABC", "XYZ"]).is_err());
                            let _ = context;
                            Ok(())
                        })
                },
            ),
        )
        .unwrap();
    }
}

mod balances {
    use super::*;
    use interledger_service::AccountStore;
    use interledger_service_util::BalanceStore;

    #[test]
    fn get_balance() {
        block_on(test_store().and_then(|(store, context)| {
            context
                .async_connection()
                .map_err(|err| panic!(err))
                .and_then(|connection| {
                    redis::cmd("HSET")
                        .arg("balances:xyz")
                        .arg(0)
                        .arg(1000)
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
    fn updating_and_rolling_back() {
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
                        .update_balances(accounts[0].clone(), 100, accounts[1].clone(), 500)
                        .and_then(move |_| {
                            store_clone_1
                                .clone()
                                .get_balance(accounts[0].clone())
                                .join(store_clone_1.clone().get_balance(accounts[1].clone()))
                                .and_then(|(balance0, balance1)| {
                                    assert_eq!(balance0, -100);
                                    assert_eq!(balance1, 500);
                                    Ok(())
                                })
                        })
                        .and_then(move |_| {
                            store_clone_2
                                .clone()
                                .undo_balance_update(account0.clone(), 100, account1.clone(), 500)
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
                        .update_balances(accounts[0].clone(), 10000, accounts[1].clone(), 500)
                        .then(move |result| {
                            assert!(result.is_err());
                            let _ = context;
                            Ok(())
                        })
                })
        }))
        .unwrap()
    }
}

mod from_btp {
    use super::*;
    use interledger_btp::{BtpAccount, BtpStore};
    use interledger_http::HttpAccount;
    use interledger_service::Account as AccountTrait;

    #[test]
    fn gets_account_from_btp_token() {
        block_on(test_store().and_then(|(store, context)| {
            store
                .get_account_from_btp_token("other_btp_token")
                .and_then(move |account| {
                    assert_eq!(account.id(), 1);
                    let _ = context;
                    Ok(())
                })
        }))
        .unwrap()
    }

    #[test]
    fn decrypts_outgoing_tokens() {
        block_on(test_store().and_then(|(store, context)| {
            store
                .get_account_from_btp_token("other_btp_token")
                .and_then(move |account| {
                    assert_eq!(
                        account.get_http_auth_token().unwrap(),
                        "outgoing_auth_token"
                    );
                    assert_eq!(
                        account.get_btp_token().unwrap().as_ref(),
                        b"other_outgoing_btp_token"
                    );
                    let _ = context;
                    Ok(())
                })
        }))
        .unwrap()
    }

    #[test]
    fn errors_on_unknown_btp_token() {
        let result = block_on(test_store().and_then(|(store, context)| {
            store
                .get_account_from_btp_token("unknown_btp_token")
                .then(move |result| {
                    let _ = context;
                    result
                })
        }));
        assert!(result.is_err());
    }
}

mod from_http {
    use super::*;
    use interledger_btp::BtpAccount;
    use interledger_http::{HttpAccount, HttpStore};
    use interledger_service::Account as AccountTrait;

    #[test]
    fn gets_account_from_http_bearer_token() {
        block_on(test_store().and_then(|(store, context)| {
            store
                .get_account_from_http_token("incoming_auth_token")
                .and_then(move |account| {
                    assert_eq!(account.id(), 0);
                    assert_eq!(
                        account.get_http_auth_token().unwrap(),
                        "outgoing_auth_token"
                    );
                    assert_eq!(account.get_btp_token().unwrap().as_ref(), b"btp_token");
                    let _ = context;
                    Ok(())
                })
        }))
        .unwrap()
    }

    #[test]
    fn decrypts_outgoing_tokens() {
        block_on(test_store().and_then(|(store, context)| {
            store
                .get_account_from_http_token("incoming_auth_token")
                .and_then(move |account| {
                    assert_eq!(
                        account.get_http_auth_token().unwrap(),
                        "outgoing_auth_token"
                    );
                    assert_eq!(account.get_btp_token().unwrap().as_ref(), b"btp_token");
                    let _ = context;
                    Ok(())
                })
        }))
        .unwrap()
    }

    #[test]
    fn errors_on_unknown_http_auth() {
        let result = block_on(test_store().and_then(|(store, context)| {
            store
                .get_account_from_http_token("unknown_token")
                .then(move |result| {
                    let _ = context;
                    result
                })
        }));
        assert!(result.is_err());
    }
}

mod ccp_store {
    use super::*;
    use interledger_ccp::RouteManagerStore;
    use interledger_router::RouterStore;
    use interledger_service::Account as AccountTrait;

    #[test]
    fn gets_accounts_to_send_routes_to() {
        block_on(test_store().and_then(|(store, context)| {
            store
                .get_accounts_to_send_routes_to()
                .and_then(move |accounts| {
                    assert_eq!(accounts[0].id(), 1);
                    assert_eq!(accounts.len(), 1);
                    let _ = context;
                    Ok(())
                })
        }))
        .unwrap()
    }

    #[test]
    fn gets_local_and_configured_routes() {
        block_on(test_store().and_then(|(store, context)| {
            store
                .get_local_and_configured_routes()
                .and_then(move |(local, configured)| {
                    assert_eq!(local.len(), 2);
                    assert!(configured.is_empty());
                    let _ = context;
                    Ok(())
                })
        }))
        .unwrap()
    }

    #[test]
    fn saves_routes_to_db() {
        block_on(test_store().and_then(|(mut store, context)| {
            let get_connection = context.async_connection();
            let account0 = Account::try_from(0, ACCOUNT_DETAILS_0.clone()).unwrap();
            let account1 = Account::try_from(1, ACCOUNT_DETAILS_1.clone()).unwrap();
            store
                .set_routes(vec![
                    (Bytes::from("example.a"), account0.clone()),
                    (Bytes::from("example.b"), account0.clone()),
                    (Bytes::from("example.c"), account1.clone()),
                ])
                .and_then(move |_| {
                    get_connection.and_then(|connection| {
                        redis::cmd("HGETALL")
                            .arg("routes")
                            .query_async(connection)
                            .map_err(|err| panic!(err))
                            .and_then(|(_conn, routes): (_, HashMap<String, u64>)| {
                                assert_eq!(routes["example.a"], 0);
                                assert_eq!(routes["example.b"], 0);
                                assert_eq!(routes["example.c"], 1);
                                assert_eq!(routes.len(), 3);
                                Ok(())
                            })
                    })
                })
                .and_then(move |_| {
                    let _ = context;
                    Ok(())
                })
        }))
        .unwrap()
    }

    #[test]
    fn updates_local_routes() {
        block_on(test_store().and_then(|(store, context)| {
            let account0 = Account::try_from(0, ACCOUNT_DETAILS_0.clone()).unwrap();
            let account1 = Account::try_from(1, ACCOUNT_DETAILS_1.clone()).unwrap();
            store
                .clone()
                .set_routes(vec![
                    (Bytes::from("example.a"), account0.clone()),
                    (Bytes::from("example.b"), account0.clone()),
                    (Bytes::from("example.c"), account1.clone()),
                ])
                .and_then(move |_| {
                    let routes = store.routing_table();
                    assert_eq!(routes[&b"example.a"[..]], 0);
                    assert_eq!(routes[&b"example.b"[..]], 0);
                    assert_eq!(routes[&b"example.c"[..]], 1);
                    assert_eq!(routes.len(), 3);
                    Ok(())
                })
                .and_then(move |_| {
                    let _ = context;
                    Ok(())
                })
        }))
        .unwrap()
    }
}

mod configured_routes {
    use super::*;
    use interledger_api::NodeStore;
    use interledger_ccp::RouteManagerStore;
    use interledger_router::RouterStore;
    use interledger_service::Account as AccountTrait;

    #[test]
    fn adds_static_routes_to_redis() {
        block_on(test_store().and_then(|(store, context)| {
            let get_connection = context.async_connection();
            store
                .clone()
                .set_static_routes(vec![
                    ("example.a".to_string(), 0),
                    ("example.b".to_string(), 0),
                    ("example.c".to_string(), 1),
                ])
                .and_then(move |_| {
                    get_connection.and_then(|connection| {
                        redis::cmd("HGETALL")
                            .arg("routes:static")
                            .query_async(connection)
                            .map_err(|err| panic!(err))
                            .and_then(move |(_, routes): (_, HashMap<String, u64>)| {
                                assert_eq!(routes["example.a"], 0);
                                assert_eq!(routes["example.b"], 0);
                                assert_eq!(routes["example.c"], 1);
                                assert_eq!(routes.len(), 3);
                                let _ = context;
                                Ok(())
                            })
                    })
                })
        }))
        .unwrap()
    }

    #[test]
    fn static_routes_override_others() {
        block_on(test_store().and_then(|(store, context)| {
            let mut store_clone = store.clone();
            store
                .clone()
                .set_static_routes(vec![
                    ("example.a".to_string(), 0),
                    ("example.b".to_string(), 0),
                ])
                .and_then(move |_| {
                    let account1 = Account::try_from(1, ACCOUNT_DETAILS_1.clone()).unwrap();
                    store_clone.set_routes(vec![
                        (Bytes::from("example.a"), account1.clone()),
                        (Bytes::from("example.b"), account1.clone()),
                        (Bytes::from("example.c"), account1),
                    ])
                })
                .and_then(move |_| {
                    let routes = store.routing_table();
                    assert_eq!(routes[&b"example.a"[..]], 0);
                    assert_eq!(routes[&b"example.b"[..]], 0);
                    assert_eq!(routes[&b"example.c"[..]], 1);
                    assert_eq!(routes.len(), 3);
                    let _ = context;
                    Ok(())
                })
        }))
        .unwrap()
    }

    #[test]
    fn returns_configured_routes_for_route_manager() {
        block_on(test_store().and_then(|(store, context)| {
            store
                .clone()
                .set_static_routes(vec![
                    ("example.a".to_string(), 0),
                    ("example.b".to_string(), 1),
                ])
                .and_then(move |_| store.get_local_and_configured_routes())
                .and_then(move |(_local, configured)| {
                    assert_eq!(configured.len(), 2);
                    assert_eq!(configured[&b"example.a"[..]].id(), 0);
                    assert_eq!(configured[&b"example.b"[..]].id(), 1);
                    let _ = context;
                    Ok(())
                })
        }))
        .unwrap()
    }
}

mod rate_limiting {
    use super::*;
    use futures::future::join_all;
    use interledger_service_util::{RateLimitError, RateLimitStore};

    #[test]
    fn rate_limits_number_of_packets() {
        block_on(test_store().and_then(|(store, context)| {
            let account = Account::try_from(0, ACCOUNT_DETAILS_0.clone()).unwrap();
            join_all(vec![
                store.clone().apply_rate_limits(account.clone(), 10),
                store.clone().apply_rate_limits(account.clone(), 10),
                store.clone().apply_rate_limits(account.clone(), 10),
            ])
            .then(move |result| {
                assert!(result.is_err());
                assert_eq!(result.unwrap_err(), RateLimitError::PacketLimitExceeded);
                let _ = context;
                Ok(())
            })
        }))
        .unwrap()
    }

    #[test]
    fn limits_amount_throughput() {
        block_on(test_store().and_then(|(store, context)| {
            let account = Account::try_from(1, ACCOUNT_DETAILS_1.clone()).unwrap();
            join_all(vec![
                store.clone().apply_rate_limits(account.clone(), 500),
                store.clone().apply_rate_limits(account.clone(), 500),
                store.clone().apply_rate_limits(account.clone(), 1),
            ])
            .then(move |result| {
                assert!(result.is_err());
                assert_eq!(result.unwrap_err(), RateLimitError::ThroughputLimitExceeded);
                let _ = context;
                Ok(())
            })
        }))
        .unwrap()
    }

    #[test]
    fn refunds_throughput_limit_for_rejected_packets() {
        block_on(test_store().and_then(|(store, context)| {
            let account = Account::try_from(1, ACCOUNT_DETAILS_1.clone()).unwrap();
            join_all(vec![
                store.clone().apply_rate_limits(account.clone(), 500),
                store.clone().apply_rate_limits(account.clone(), 500),
            ])
            .map_err(|err| panic!(err))
            .and_then(move |_| {
                let store_clone = store.clone();
                let account_clone = account.clone();
                store
                    .clone()
                    .refund_throughput_limit(account.clone(), 500)
                    .and_then(move |_| {
                        store
                            .clone()
                            .apply_rate_limits(account.clone(), 500)
                            .map_err(|err| panic!(err))
                    })
                    .and_then(move |_| {
                        store_clone
                            .apply_rate_limits(account_clone, 1)
                            .then(move |result| {
                                assert!(result.is_err());
                                assert_eq!(
                                    result.unwrap_err(),
                                    RateLimitError::ThroughputLimitExceeded
                                );
                                let _ = context;
                                Ok(())
                            })
                    })
            })
        }))
        .unwrap()
    }
}
