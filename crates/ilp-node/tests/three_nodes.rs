use futures::{future::join_all, Future};
use ilp_node::InterledgerNode;
use serde_json::json;
use tokio::runtime::Builder as RuntimeBuilder;
use tracing::error_span;
use tracing_futures::Instrument;

mod redis_helpers;
use redis_helpers::*;

mod test_helpers;
use interledger::packet::Address;
use interledger::stream::StreamDelivery;
use std::str::FromStr;
use test_helpers::*;

#[test]
fn three_nodes() {
    // Nodes 1 and 2 are peers, Node 2 is the parent of Node 3
    install_tracing_subscriber();
    let context = TestContext::new();

    // Each node will use its own DB within the redis instance
    let mut connection_info1 = context.get_client_connection_info();
    connection_info1.db = 1;
    let mut connection_info2 = context.get_client_connection_info();
    connection_info2.db = 2;
    let mut connection_info3 = context.get_client_connection_info();
    connection_info3.db = 3;

    let node1_http = get_open_port(Some(3010));
    let node1_settlement = get_open_port(Some(3011));
    let node2_http = get_open_port(Some(3020));
    let node2_settlement = get_open_port(Some(3021));
    let node3_http = get_open_port(Some(3030));
    let node3_settlement = get_open_port(Some(3031));

    let mut runtime = RuntimeBuilder::new()
        .panic_handler(|_| panic!("Tokio worker panicked"))
        .build()
        .unwrap();

    let alice_on_alice = json!({
        "ilp_address": "example.alice",
        "username": "alice",
        "asset_code": "XYZ",
        "asset_scale": 9,
        "ilp_over_http_incoming_token" : "default account holder",
    });
    let bob_on_alice = json!({
        "ilp_address": "example.bob",
        "username": "bob",
        "asset_code": "XYZ",
        "asset_scale": 9,
        "ilp_over_http_url": format!("http://localhost:{}/ilp", node2_http),
        "ilp_over_http_incoming_token" : "two",
        "ilp_over_http_outgoing_token" : "alice:one",
        "min_balance": -1_000_000_000,
        "routing_relation": "Peer",
    });

    let alice_on_bob = json!({
        "ilp_address": "example.alice",
        "username": "alice",
        "asset_code": "XYZ",
        "asset_scale": 9,
        "ilp_over_http_url": format!("http://localhost:{}/ilp", node1_http),
        "ilp_over_http_incoming_token" : "one",
        "ilp_over_http_outgoing_token" : "bob:two",
        "routing_relation": "Peer",
    });
    let charlie_on_bob = json!({
        "username": "charlie",
        "asset_code": "ABC",
        "asset_scale": 6,
        "ilp_over_btp_incoming_token" : "three",
        "ilp_over_http_incoming_token" : "three",
        "min_balance": -1_000_000_000,
        "routing_relation": "Child",
    });

    let charlie_on_charlie = json!({
        "username": "charlie",
        "asset_code": "ABC",
        "asset_scale": 6,
        "ilp_over_http_incoming_token" : "default account holder",
    });
    let bob_on_charlie = json!({
        "ilp_address": "example.bob",
        "username": "bob",
        "asset_code": "ABC",
        "asset_scale": 6,
        "ilp_over_http_incoming_token" : "two",
        "ilp_over_http_outgoing_token": "charlie:three",
        "ilp_over_http_url": format!("http://localhost:{}/ilp", node2_http),
        "ilp_over_btp_url": format!("btp+ws://localhost:{}/ilp/btp", node2_http),
        "ilp_over_btp_outgoing_token": "charlie:three",
        "min_balance": -1_000_000_000,
        "routing_relation": "Parent",
    });

    let node1: InterledgerNode = serde_json::from_value(json!({
        "ilp_address": "example.alice",
        "default_spsp_account": "alice",
        "admin_auth_token": "admin",
        "redis_connection": connection_info_to_string(connection_info1),
        "http_bind_address": format!("127.0.0.1:{}", node1_http),
        "settlement_api_bind_address": format!("127.0.0.1:{}", node1_settlement),
        "secret_seed": random_secret(),
        "route_broadcast_interval": Some(200),
        "exchange_rate_poll_interval": 60000,
    }))
    .expect("Error creating node1.");

    let node2: InterledgerNode = serde_json::from_value(json!({
        "ilp_address": "example.bob",
        "default_spsp_account": "bob",
        "admin_auth_token": "admin",
        "redis_connection": connection_info_to_string(connection_info2),
        "http_bind_address": format!("127.0.0.1:{}", node2_http),
        "settlement_api_bind_address": format!("127.0.0.1:{}", node2_settlement),
        "secret_seed": random_secret(),
        "route_broadcast_interval": Some(200),
        "exchange_rate_poll_interval": 60000,
    }))
    .expect("Error creating node2.");

    let node3: InterledgerNode = serde_json::from_value(json!({
        "default_spsp_account": "charlie",
        "admin_auth_token": "admin",
        "redis_connection": connection_info_to_string(connection_info3),
        "http_bind_address": format!("127.0.0.1:{}", node3_http),
        "settlement_api_bind_address": format!("127.0.0.1:{}", node3_settlement),
        "secret_seed": random_secret(),
        "route_broadcast_interval": Some(200),
        "exchange_rate_poll_interval": 60000,
    }))
    .expect("Error creating node3.");

    let alice_fut = join_all(vec![
        create_account_on_node(node1_http, alice_on_alice, "admin"),
        create_account_on_node(node1_http, bob_on_alice, "admin"),
    ]);

    runtime.spawn(
        node1
            .serve()
            .and_then(move |_| alice_fut)
            .and_then(|_| Ok(()))
            .instrument(error_span!(target: "interledger", "node1")),
    );

    let bob_fut = join_all(vec![
        create_account_on_node(node2_http, alice_on_bob, "admin"),
        create_account_on_node(node2_http, charlie_on_bob, "admin"),
    ]);

    runtime.spawn(
        node2
            .serve()
            .and_then(move |_| bob_fut)
            .and_then(move |_| {
                let client = reqwest::r#async::Client::new();
                client
                    .put(&format!("http://localhost:{}/rates", node2_http))
                    .header("Authorization", "Bearer admin")
                    .json(&json!({"ABC": 2, "XYZ": 1}))
                    .send()
                    .map_err(|err| panic!(err))
                    .and_then(|res| {
                        res.error_for_status()
                            .expect("Error setting exchange rates");
                        Ok(())
                    })
            })
            .instrument(error_span!(target: "interledger", "node2")),
    );

    // We execute the futures one after the other to avoid race conditions where
    // Bob gets added before the node's main account
    let charlie_fut = create_account_on_node(node3_http, charlie_on_charlie, "admin")
        .and_then(move |_| create_account_on_node(node3_http, bob_on_charlie, "admin"));

    runtime
        .block_on(
            node3
                .serve()
                .and_then(move |_| charlie_fut)
                .instrument(error_span!(target: "interledger", "node3"))
                // we wait some time after the node is up so that we get the
                // necessary routes from bob
                .and_then(move |_| {
                    delay(1000).map_err(|_| panic!("Something strange happened when `delay`"))
                })
                .and_then(move |_| {
                    let send_1_to_3 = send_money_to_username(
                        node1_http,
                        node3_http,
                        1000,
                        "charlie",
                        "alice",
                        "default account holder",
                    );
                    let send_3_to_1 = send_money_to_username(
                        node3_http,
                        node1_http,
                        1000,
                        "alice",
                        "charlie",
                        "default account holder",
                    );

                    let get_balances = move || {
                        futures::future::join_all(vec![
                            get_balance("alice", node1_http, "admin"),
                            get_balance("charlie", node2_http, "admin"),
                            get_balance("charlie", node3_http, "admin"),
                        ])
                    };

                    // Node 1 sends 1000 to Node 3. However, Node1's scale is 9,
                    // while Node 3's scale is 6. This means that Node 3 will
                    // see 1000x less. In addition, the conversion rate is 2:1
                    // for 3's asset, so he will receive 2 total.
                    send_1_to_3
                        .map_err(|err| {
                            eprintln!("Error sending from node 1 to node 3: {:?}", err);
                            err
                        })
                        .and_then(move |receipt: StreamDelivery| {
                            assert_eq!(receipt.from, Address::from_str("example.alice").unwrap());
                            assert!(receipt.to.to_string().starts_with("example.bob.charlie"));
                            assert_eq!(receipt.sent_asset_code, "XYZ");
                            assert_eq!(receipt.sent_asset_scale, 9);
                            assert_eq!(receipt.sent_amount, 1000);
                            assert_eq!(receipt.delivered_asset_code.unwrap(), "ABC");
                            assert_eq!(receipt.delivered_amount, 2);
                            assert_eq!(receipt.delivered_asset_scale.unwrap(), 6);
                            get_balances().and_then(move |ret| {
                                assert_eq!(ret[0], -1000);
                                assert_eq!(ret[1], 2);
                                assert_eq!(ret[2], 2);
                                Ok(())
                            })
                        })
                        .and_then(move |_| {
                            send_3_to_1.map_err(|err| {
                                eprintln!("Error sending from node 3 to node 1: {:?}", err);
                                err
                            })
                        })
                        .and_then(move |receipt| {
                            assert_eq!(
                                receipt.from,
                                Address::from_str("example.bob.charlie").unwrap()
                            );
                            assert!(receipt.to.to_string().starts_with("example.alice"));
                            assert_eq!(receipt.sent_asset_code, "ABC");
                            assert_eq!(receipt.sent_asset_scale, 6);
                            assert_eq!(receipt.sent_amount, 1000);
                            assert_eq!(receipt.delivered_asset_code.unwrap(), "XYZ");
                            assert_eq!(receipt.delivered_amount, 500_000);
                            assert_eq!(receipt.delivered_asset_scale.unwrap(), 9);
                            get_balances().and_then(move |ret| {
                                assert_eq!(ret[0], 499_000);
                                assert_eq!(ret[1], -998);
                                assert_eq!(ret[2], -998);
                                Ok(())
                            })
                        })
                }),
        )
        .map_err(|err| {
            eprintln!("Error executing tests: {:?}", err);
            err
        })
        .unwrap();
}
