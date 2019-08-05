mod common;

use bytes::Bytes;
use common::*;
use interledger_api::{AccountDetails, NodeStore};
use interledger_ccp::RouteManagerStore;
use interledger_packet::Address;
use interledger_router::RouterStore;
use interledger_service::Account as AccountTrait;
use std::str::FromStr;
use std::{collections::HashMap, time::Duration};
use tokio_timer::sleep;

#[test]
fn polls_for_route_updates() {
    let context = TestContext::new();
    block_on(
        RedisStoreBuilder::new(context.get_client_connection_info(), [0; 32])
            .poll_interval(1)
            .connect()
            .and_then(|store| {
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
                            ilp_address: Address::from_str("example.bob").unwrap(),
                            asset_scale: 6,
                            asset_code: "XYZ".to_string(),
                            max_packet_amount: 1000,
                            min_balance: Some(-1000),
                            http_endpoint: None,
                            http_incoming_token: None,
                            http_outgoing_token: None,
                            btp_uri: None,
                            btp_incoming_token: None,
                            settle_threshold: None,
                            settle_to: None,
                            send_routes: false,
                            receive_routes: false,
                            routing_relation: None,
                            round_trip_time: None,
                            amount_per_minute_limit: None,
                            packets_per_minute_limit: None,
                            settlement_engine_url: None,
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
                                    .arg("routes:current")
                                    .arg("example.alice")
                                    .arg(1)
                                    .arg("example.charlie")
                                    .arg(0)
                                    .query_async(connection)
                                    .and_then(|(_connection, _result): (_, redis::Value)| Ok(()))
                                    .map_err(|err| panic!(err))
                                    .and_then(|_| sleep(Duration::from_millis(10)).then(|_| Ok(())))
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
                                    *routing_table.get(&Bytes::from("example.charlie")).unwrap(),
                                    0
                                );
                                assert!(routing_table.get(&Bytes::from("example.other")).is_none());
                                let _ = context;
                                Ok(())
                            })
                    })
            }),
    )
    .unwrap();
}

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
fn gets_accounts_to_receive_routes_from() {
    block_on(test_store().and_then(|(store, context)| {
        store
            .get_accounts_to_receive_routes_from()
            .and_then(move |accounts| {
                assert_eq!(accounts[0].id(), 0);
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
                        .arg("routes:current")
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
