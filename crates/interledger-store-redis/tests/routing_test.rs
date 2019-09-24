mod common;

use bytes::Bytes;
use common::*;
use interledger_api::{AccountDetails, NodeStore};
use interledger_ccp::RouteManagerStore;
use interledger_packet::Address;
use interledger_router::RouterStore;
use interledger_service::{Account as AccountTrait, AddressStore, Username};
use interledger_store_redis::AccountId;
use std::str::FromStr;
use std::{collections::HashMap, time::Duration};
use tokio_timer::sleep;

#[test]
fn polls_for_route_updates() {
    let context = TestContext::new();
    block_on(
        RedisStoreBuilder::new(context.get_client_connection_info(), [0; 32])
            .poll_interval(1)
            .node_ilp_address(Address::from_str("example.node").unwrap())
            .connect()
            .and_then(|store| {
                let connection = context.async_connection();
                assert_eq!(store.routing_table().len(), 0);
                let store_clone_1 = store.clone();
                let store_clone_2 = store.clone();
                store
                    .clone()
                    .insert_account(ACCOUNT_DETAILS_0.clone())
                    .and_then(move |alice| {
                        let routing_table = store_clone_1.routing_table();
                        assert_eq!(routing_table.len(), 1);
                        assert_eq!(
                            *routing_table.get(&Bytes::from("example.alice")).unwrap(),
                            alice.id()
                        );
                        store_clone_1
                            .insert_account(AccountDetails {
                                ilp_address: Some(Address::from_str("example.bob").unwrap()),
                                username: Username::from_str("bob").unwrap(),
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
                                routing_relation: Some("Peer".to_owned()),
                                round_trip_time: None,
                                amount_per_minute_limit: None,
                                packets_per_minute_limit: None,
                                settlement_engine_url: None,
                            })
                            .and_then(move |bob| {
                                let routing_table = store_clone_2.routing_table();
                                assert_eq!(routing_table.len(), 2);
                                assert_eq!(
                                    *routing_table.get(&Bytes::from("example.bob")).unwrap(),
                                    bob.id(),
                                );
                                let alice_id = alice.id();
                                let bob_id = bob.id();
                                connection
                                    .map_err(|err| panic!(err))
                                    .and_then(move |connection| {
                                        redis::cmd("HMSET")
                                            .arg("routes:current")
                                            .arg("example.alice")
                                            .arg(bob_id)
                                            .arg("example.charlie")
                                            .arg(alice_id)
                                            .query_async(connection)
                                            .and_then(
                                                |(_connection, _result): (_, redis::Value)| Ok(()),
                                            )
                                            .map_err(|err| panic!(err))
                                            .and_then(|_| {
                                                sleep(Duration::from_millis(10)).then(|_| Ok(()))
                                            })
                                    })
                                    .and_then(move |_| {
                                        let routing_table = store_clone_2.routing_table();
                                        assert_eq!(routing_table.len(), 3);
                                        assert_eq!(
                                            *routing_table
                                                .get(&Bytes::from("example.alice"))
                                                .unwrap(),
                                            bob_id
                                        );
                                        assert_eq!(
                                            *routing_table
                                                .get(&Bytes::from("example.bob"))
                                                .unwrap(),
                                            bob.id(),
                                        );
                                        assert_eq!(
                                            *routing_table
                                                .get(&Bytes::from("example.charlie"))
                                                .unwrap(),
                                            alice_id,
                                        );
                                        assert!(routing_table
                                            .get(&Bytes::from("example.other"))
                                            .is_none());
                                        let _ = context;
                                        Ok(())
                                    })
                            })
                    })
            }),
    )
    .unwrap();
}

#[test]
fn gets_accounts_to_send_routes_to() {
    block_on(test_store().and_then(|(store, context, _accs)| {
        store
            .get_accounts_to_send_routes_to()
            .and_then(move |accounts| {
                assert_eq!(
                    *accounts[0].ilp_address(),
                    Address::from_str("example.alice.user1.bob").unwrap()
                );
                assert_eq!(accounts.len(), 1);
                let _ = context;
                Ok(())
            })
    }))
    .unwrap()
}

#[test]
fn gets_accounts_to_receive_routes_from() {
    block_on(test_store().and_then(|(store, context, _accs)| {
        store
            .get_accounts_to_receive_routes_from()
            .and_then(move |accounts| {
                assert_eq!(
                    *accounts[0].ilp_address(),
                    Address::from_str("example.alice").unwrap()
                );
                assert_eq!(accounts.len(), 1);
                let _ = context;
                Ok(())
            })
    }))
    .unwrap()
}

#[test]
fn gets_local_and_configured_routes() {
    block_on(test_store().and_then(|(store, context, _accs)| {
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
    block_on(test_store().and_then(|(mut store, context, _accs)| {
        let get_connection = context.async_connection();
        let account0_id = AccountId::new();
        let account1_id = AccountId::new();
        let account0 = Account::try_from(
            account0_id,
            ACCOUNT_DETAILS_0.clone(),
            store.get_ilp_address(),
        )
        .unwrap();

        let account1 = Account::try_from(
            account1_id,
            ACCOUNT_DETAILS_1.clone(),
            store.get_ilp_address(),
        )
        .unwrap();

        store
            .set_routes(vec![
                (Bytes::from("example.a"), account0.clone()),
                (Bytes::from("example.b"), account0.clone()),
                (Bytes::from("example.c"), account1.clone()),
            ])
            .and_then(move |_| {
                get_connection.and_then(move |connection| {
                    redis::cmd("HGETALL")
                        .arg("routes:current")
                        .query_async(connection)
                        .map_err(|err| panic!(err))
                        .and_then(move |(_conn, routes): (_, HashMap<String, AccountId>)| {
                            assert_eq!(routes["example.a"], account0_id);
                            assert_eq!(routes["example.b"], account0_id);
                            assert_eq!(routes["example.c"], account1_id);
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
    block_on(test_store().and_then(|(store, context, _accs)| {
        let account0_id = AccountId::new();
        let account1_id = AccountId::new();
        let account0 = Account::try_from(
            account0_id,
            ACCOUNT_DETAILS_0.clone(),
            store.get_ilp_address(),
        )
        .unwrap();
        let account1 = Account::try_from(
            account1_id,
            ACCOUNT_DETAILS_1.clone(),
            store.get_ilp_address(),
        )
        .unwrap();
        store
            .clone()
            .set_routes(vec![
                (Bytes::from("example.a"), account0.clone()),
                (Bytes::from("example.b"), account0.clone()),
                (Bytes::from("example.c"), account1.clone()),
            ])
            .and_then(move |_| {
                let routes = store.routing_table();
                assert_eq!(routes[&b"example.a"[..]], account0_id);
                assert_eq!(routes[&b"example.b"[..]], account0_id);
                assert_eq!(routes[&b"example.c"[..]], account1_id);
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
    block_on(test_store().and_then(|(store, context, accs)| {
        let get_connection = context.async_connection();
        store
            .clone()
            .set_static_routes(vec![
                ("example.a".to_string(), accs[0].id()),
                ("example.b".to_string(), accs[0].id()),
                ("example.c".to_string(), accs[1].id()),
            ])
            .and_then(move |_| {
                get_connection.and_then(|connection| {
                    redis::cmd("HGETALL")
                        .arg("routes:static")
                        .query_async(connection)
                        .map_err(|err| panic!(err))
                        .and_then(move |(_, routes): (_, HashMap<String, AccountId>)| {
                            assert_eq!(routes["example.a"], accs[0].id());
                            assert_eq!(routes["example.b"], accs[0].id());
                            assert_eq!(routes["example.c"], accs[1].id());
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
    block_on(test_store().and_then(|(store, context, accs)| {
        let mut store_clone = store.clone();
        store
            .clone()
            .set_static_routes(vec![
                ("example.a".to_string(), accs[0].id()),
                ("example.b".to_string(), accs[0].id()),
            ])
            .and_then(move |_| {
                let account1_id = AccountId::new();
                let account1 = Account::try_from(
                    account1_id,
                    ACCOUNT_DETAILS_1.clone(),
                    store.get_ilp_address(),
                )
                .unwrap();
                store_clone
                    .set_routes(vec![
                        (Bytes::from("example.a"), account1.clone()),
                        (Bytes::from("example.b"), account1.clone()),
                        (Bytes::from("example.c"), account1),
                    ])
                    .and_then(move |_| {
                        let routes = store.routing_table();
                        assert_eq!(routes[&b"example.a"[..]], accs[0].id());
                        assert_eq!(routes[&b"example.b"[..]], accs[0].id());
                        assert_eq!(routes[&b"example.c"[..]], account1_id);
                        assert_eq!(routes.len(), 3);
                        let _ = context;
                        Ok(())
                    })
            })
    }))
    .unwrap()
}

#[test]
fn returns_configured_routes_for_route_manager() {
    block_on(test_store().and_then(|(store, context, accs)| {
        store
            .clone()
            .set_static_routes(vec![
                ("example.a".to_string(), accs[0].id()),
                ("example.b".to_string(), accs[1].id()),
            ])
            .and_then(move |_| store.get_local_and_configured_routes())
            .and_then(move |(_local, configured)| {
                assert_eq!(configured.len(), 2);
                assert_eq!(configured[&b"example.a"[..]].id(), accs[0].id());
                assert_eq!(configured[&b"example.b"[..]].id(), accs[1].id());
                let _ = context;
                Ok(())
            })
    }))
    .unwrap()
}
