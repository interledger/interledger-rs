use super::{fixtures::*, redis_helpers::*, store_helpers::*};

use interledger_api::{AccountDetails, NodeStore};
use interledger_ccp::CcpRoutingStore;
use interledger_packet::Address;
use interledger_router::RouterStore;
use interledger_service::{Account as AccountTrait, AddressStore, Username};
use interledger_store::{account::Account, redis::RedisStoreBuilder};
use std::str::FromStr;
use std::{collections::HashMap, time::Duration};
use uuid::Uuid;

#[tokio::test]
async fn polls_for_route_updates() {
    let context = TestContext::new();
    let store = RedisStoreBuilder::new(context.get_client_connection_info(), [0; 32])
        .poll_interval(1)
        .node_ilp_address(Address::from_str("example.node").unwrap())
        .connect()
        .await
        .unwrap();

    let connection = context.async_connection();
    assert_eq!(store.routing_table().len(), 0);
    let store_clone_1 = store.clone();
    let store_clone_2 = store.clone();
    let alice = store
        .insert_account(ACCOUNT_DETAILS_0.clone())
        .await
        .unwrap();
    let routing_table = store_clone_1.routing_table();
    assert_eq!(routing_table.len(), 1);
    assert_eq!(*routing_table.get("example.alice").unwrap(), alice.id());
    let bob = store_clone_1
        .insert_account(AccountDetails {
            ilp_address: Some(Address::from_str("example.bob").unwrap()),
            username: Username::from_str("bob").unwrap(),
            asset_scale: 6,
            asset_code: "XYZ".to_string(),
            max_packet_amount: 1000,
            min_balance: Some(-1000),
            ilp_over_http_url: None,
            ilp_over_http_incoming_token: None,
            ilp_over_http_outgoing_token: None,
            ilp_over_btp_url: None,
            ilp_over_btp_outgoing_token: None,
            ilp_over_btp_incoming_token: None,
            settle_threshold: None,
            settle_to: None,
            routing_relation: Some("Peer".to_owned()),
            round_trip_time: None,
            amount_per_minute_limit: None,
            packets_per_minute_limit: None,
            settlement_engine_url: None,
        })
        .await
        .unwrap();

    let routing_table = store_clone_2.routing_table();
    assert_eq!(routing_table.len(), 2);
    assert_eq!(*routing_table.get("example.bob").unwrap(), bob.id(),);
    let alice_id = alice.id();
    let bob_id = bob.id();
    let mut connection = connection.await.unwrap();
    let _: redis_crate::Value = redis_crate::cmd("HMSET")
        .arg("routes:current")
        .arg("example.alice")
        .arg(bob_id.to_string())
        .arg("example.charlie")
        .arg(alice_id.to_string())
        .query_async(&mut connection)
        .await
        .unwrap();

    tokio::time::sleep(Duration::from_millis(10)).await;
    let routing_table = store_clone_2.routing_table();
    assert_eq!(routing_table.len(), 3);
    assert_eq!(*routing_table.get("example.alice").unwrap(), bob_id);
    assert_eq!(*routing_table.get("example.bob").unwrap(), bob.id(),);
    assert_eq!(*routing_table.get("example.charlie").unwrap(), alice_id,);
    assert!(routing_table.get("example.other").is_none());
}

#[tokio::test]
async fn gets_accounts_to_send_routes_to() {
    let (store, _context, _) = test_store().await.unwrap();
    let accounts = store
        .get_accounts_to_send_routes_to(Vec::new())
        .await
        .unwrap();
    // We send to child accounts but not parents
    assert_eq!(accounts[0].username().as_ref(), "bob");
    assert_eq!(accounts.len(), 1);
}

#[tokio::test]
async fn gets_accounts_to_send_routes_to_and_skips_ignored() {
    let (store, _context, accs) = test_store().await.unwrap();
    let accounts = store
        .get_accounts_to_send_routes_to(vec![accs[1].id()])
        .await
        .unwrap();
    assert!(accounts.is_empty());
}

#[tokio::test]
async fn gets_accounts_to_receive_routes_from() {
    let (store, _context, _) = test_store().await.unwrap();
    let accounts = store.get_accounts_to_receive_routes_from().await.unwrap();
    assert_eq!(
        *accounts[0].ilp_address(),
        Address::from_str("example.alice").unwrap()
    );
}

#[tokio::test]
async fn gets_local_and_configured_routes() {
    let (store, _context, _) = test_store().await.unwrap();
    let (local, configured) = store.get_local_and_configured_routes().await.unwrap();
    assert_eq!(local.len(), 2);
    assert!(configured.is_empty());
}

#[tokio::test]
async fn saves_routes_to_db() {
    let (store, context, _) = test_store().await.unwrap();
    let get_connection = context.async_connection();
    let account0_id = Uuid::new_v4();
    let account1_id = Uuid::new_v4();
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
            ("example.a".to_string(), account0.clone()),
            ("example.b".to_string(), account0.clone()),
            ("example.c".to_string(), account1.clone()),
        ])
        .await
        .unwrap();

    let mut connection = get_connection.await.unwrap();
    let routes: HashMap<String, String> = redis_crate::cmd("HGETALL")
        .arg("routes:current")
        .query_async(&mut connection)
        .await
        .unwrap();
    assert_eq!(routes["example.a"], account0_id.to_string());
    assert_eq!(routes["example.b"], account0_id.to_string());
    assert_eq!(routes["example.c"], account1_id.to_string());
    assert_eq!(routes.len(), 3);

    // local routing table routes are also updated
    let routes = store.routing_table();
    assert_eq!(routes["example.a"], account0_id);
    assert_eq!(routes["example.b"], account0_id);
    assert_eq!(routes["example.c"], account1_id);
    assert_eq!(routes.len(), 3);
}

#[tokio::test]
async fn adds_static_routes_to_redis() {
    let (store, context, accs) = test_store().await.unwrap();
    let get_connection = context.async_connection();
    store
        .set_static_routes(vec![
            ("example.a".to_string(), accs[0].id()),
            ("example.b".to_string(), accs[0].id()),
            ("example.c".to_string(), accs[1].id()),
        ])
        .await
        .unwrap();
    let mut connection = get_connection.await.unwrap();
    let routes: HashMap<String, String> = redis_crate::cmd("HGETALL")
        .arg("routes:static")
        .query_async(&mut connection)
        .await
        .unwrap();
    assert_eq!(routes["example.a"], accs[0].id().to_string());
    assert_eq!(routes["example.b"], accs[0].id().to_string());
    assert_eq!(routes["example.c"], accs[1].id().to_string());
    assert_eq!(routes.len(), 3);
}

#[tokio::test]
async fn static_routes_override_others() {
    let (store, _context, accs) = test_store().await.unwrap();
    store
        .set_static_routes(vec![
            ("example.a".to_string(), accs[0].id()),
            ("example.b".to_string(), accs[0].id()),
        ])
        .await
        .unwrap();

    let account1_id = Uuid::new_v4();
    let account1 = Account::try_from(
        account1_id,
        ACCOUNT_DETAILS_1.clone(),
        store.get_ilp_address(),
    )
    .unwrap();
    store
        .clone()
        .set_routes(vec![
            ("example.a".to_string(), account1.clone()),
            ("example.b".to_string(), account1.clone()),
            ("example.c".to_string(), account1),
        ])
        .await
        .unwrap();

    let routes = store.routing_table();
    assert_eq!(routes["example.a"], accs[0].id());
    assert_eq!(routes["example.b"], accs[0].id());
    assert_eq!(routes["example.c"], account1_id);
    assert_eq!(routes.len(), 3);
}

#[tokio::test]
async fn default_route() {
    let (store, _context, accs) = test_store().await.unwrap();
    store.set_default_route(accs[0].id()).await.unwrap();
    let account1_id = Uuid::new_v4();
    let account1 = Account::try_from(
        account1_id,
        ACCOUNT_DETAILS_1.clone(),
        store.get_ilp_address(),
    )
    .unwrap();
    store
        .clone()
        .set_routes(vec![
            ("example.a".to_string(), account1.clone()),
            ("example.b".to_string(), account1.clone()),
        ])
        .await
        .unwrap();

    let routes = store.routing_table();
    assert_eq!(routes[""], accs[0].id());
    assert_eq!(routes["example.a"], account1_id);
    assert_eq!(routes["example.b"], account1_id);
    assert_eq!(routes.len(), 3);
}

#[tokio::test]
async fn returns_configured_routes_for_route_manager() {
    let (store, _context, accs) = test_store().await.unwrap();
    store
        .set_static_routes(vec![
            ("example.a".to_string(), accs[0].id()),
            ("example.b".to_string(), accs[1].id()),
        ])
        .await
        .unwrap();
    let (_, configured) = store.get_local_and_configured_routes().await.unwrap();
    assert_eq!(configured.len(), 2);
    assert_eq!(configured["example.a"].id(), accs[0].id());
    assert_eq!(configured["example.b"].id(), accs[1].id());
}
