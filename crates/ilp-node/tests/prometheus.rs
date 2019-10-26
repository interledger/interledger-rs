use futures::{future::join_all, Future};
use ilp_node::InterledgerNode;
use reqwest::r#async::Client;
use serde_json::{self, json};
use tokio::runtime::Builder as RuntimeBuilder;

mod redis_helpers;
use redis_helpers::*;

mod test_helpers;
use test_helpers::*;

#[test]
fn prometheus() {
    // Nodes 1 and 2 are peers, Node 2 is the parent of Node 2
    install_tracing_subscriber();
    let context = TestContext::new();

    // Each node will use its own DB within the redis instance
    let mut connection_info1 = context.get_client_connection_info();
    connection_info1.db = 1;
    let mut connection_info2 = context.get_client_connection_info();
    connection_info2.db = 2;

    let node_a_http = get_open_port(Some(3010));
    let node_a_settlement = get_open_port(Some(3011));
    let node_b_http = get_open_port(Some(3020));
    let node_b_settlement = get_open_port(Some(3021));
    let prometheus_port = get_open_port(None);

    let mut runtime = RuntimeBuilder::new()
        .panic_handler(|err| std::panic::resume_unwind(err))
        .build()
        .unwrap();

    let alice_on_a = json!({
        "ilp_address": "example.node_a.alice",
        "username": "alice_on_a",
        "asset_code": "XYZ",
        "asset_scale": 9,
        "ilp_over_http_incoming_token" : "token",
    });
    let b_on_a = json!({
        "ilp_address": "example.node_b",
        "username": "node_b",
        "asset_code": "XYZ",
        "asset_scale": 9,
        "ilp_over_http_url": format!("http://localhost:{}/ilp", node_b_http),
        "ilp_over_http_incoming_token" : "token",
        "ilp_over_http_outgoing_token" : "node_a:token",
        "routing_relation": "Peer",
    });

    let a_on_b = json!({
        "ilp_address": "example.node_a",
        "username": "node_a",
        "asset_code": "XYZ",
        "asset_scale": 9,
        "ilp_over_http_url": format!("http://localhost:{}/ilp", node_a_http),
        "ilp_over_http_incoming_token" : "token",
        "ilp_over_http_outgoing_token" : "node_b:token",
        "routing_relation": "Peer",
    });

    let bob_on_b = json!({
        "ilp_address": "example.node_b.bob",
        "username": "bob_on_b",
        "asset_code": "XYZ",
        "asset_scale": 6,
        "ilp_over_http_incoming_token" : "token",
    });

    let node_a: InterledgerNode = serde_json::from_value(json!({
        "admin_auth_token": "admin",
        "redis_connection": connection_info_to_string(connection_info1),
        "http_bind_address": format!("127.0.0.1:{}", node_a_http),
        "settlement_api_bind_address": format!("127.0.0.1:{}", node_a_settlement),
        "secret_seed": random_secret(),
        "prometheus": {
            "bind_address": format!("127.0.0.1:{}", prometheus_port),
            "histogram_window": 10000,
            "histogram_granularity": 1000,
        }
    }))
    .unwrap();

    let node_b: InterledgerNode = serde_json::from_value(json!({
        "ilp_address": "example.parent",
        "default_spsp_account": "bob_on_b",
        "admin_auth_token": "admin",
        "redis_connection": connection_info_to_string(connection_info2),
        "http_bind_address": format!("127.0.0.1:{}", node_b_http),
        "settlement_api_bind_address": format!("127.0.0.1:{}", node_b_settlement),
        "secret_seed": random_secret(),
    }))
    .unwrap();

    let alice_fut = join_all(vec![
        create_account_on_node(node_a_http, alice_on_a, "admin"),
        create_account_on_node(node_a_http, b_on_a, "admin"),
    ]);

    runtime.spawn(node_a.serve());

    let bob_fut = join_all(vec![
        create_account_on_node(node_b_http, a_on_b, "admin"),
        create_account_on_node(node_b_http, bob_on_b, "admin"),
    ]);

    runtime.spawn(node_b.serve());

    runtime
        .block_on(
            // Wait for the nodes to spin up
            delay(500)
                .map_err(|_| panic!("Something strange happened"))
                .and_then(move |_| {
                    bob_fut
                        .and_then(|_| alice_fut)
                        .and_then(|_| delay(500).map_err(|_| panic!("delay error")))
                })
                .and_then(move |_| {
                    let send_1_to_2 = send_money_to_username(
                        node_a_http,
                        node_b_http,
                        1000,
                        "bob_on_b",
                        "alice_on_a",
                        "token",
                    );
                    let send_2_to_1 = send_money_to_username(
                        node_b_http,
                        node_a_http,
                        2000,
                        "alice_on_a",
                        "bob_on_b",
                        "token",
                    );

                    let check_metrics = move || {
                        Client::new()
                            .get(&format!("http://127.0.0.1:{}", prometheus_port))
                            .send()
                            .map_err(|err| eprintln!("Error getting metrics {:?}", err))
                            .and_then(|mut res| {
                                res.text().map_err(|err| {
                                    eprintln!("Response was not a string: {:?}", err)
                                })
                            })
                    };

                    send_1_to_2
                        .map_err(|err| {
                            eprintln!("Error sending from node 1 to node 2: {:?}", err);
                            err
                        })
                        .and_then(move |_| {
                            check_metrics().and_then(move |ret| {
                                assert!(ret.starts_with("# metrics snapshot"));
                                assert!(ret.contains("requests_incoming_fulfill"));
                                assert!(ret.contains("requests_incoming_prepare"));
                                assert!(ret.contains("requests_incoming_reject"));
                                assert!(ret.contains("requests_incoming_duration"));
                                assert!(ret.contains("requests_outgoing_fulfill"));
                                assert!(ret.contains("requests_outgoing_prepare"));
                                assert!(ret.contains("requests_outgoing_reject"));
                                assert!(ret.contains("requests_outgoing_duration"));
                                // TODO check the specific numbers of packets
                                Ok(())
                            })
                        })
                        .and_then(move |_| {
                            send_2_to_1.map_err(|err| {
                                eprintln!("Error sending from node 2 to node 1: {:?}", err);
                                err
                            })
                        })
                        .and_then(move |_| {
                            check_metrics().and_then(move |ret| {
                                assert!(ret.starts_with("# metrics snapshot"));
                                assert!(ret.contains("requests_incoming_fulfill"));
                                assert!(ret.contains("requests_incoming_prepare"));
                                assert!(ret.contains("requests_incoming_reject"));
                                assert!(ret.contains("requests_incoming_duration"));
                                assert!(ret.contains("requests_outgoing_fulfill"));
                                assert!(ret.contains("requests_outgoing_prepare"));
                                assert!(ret.contains("requests_outgoing_reject"));
                                assert!(ret.contains("requests_outgoing_duration"));
                                // TODO check the specific numbers of packets
                                Ok(())
                            })
                        })
                }),
        )
        .unwrap();
}
