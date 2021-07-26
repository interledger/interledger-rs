//! Multiple payments benchmark
//! Adds four bench functions which tracks, how long it takes for an ilp-node to receive payment through websocket, process it and respond back.
//! Half of the benchmarks are with http and half are with btp.
//! Half of the benchmarks are with high packet count (100) and half are with single packets.
//! Sometimes it might take a while to start the benchmarks as the route propagation start up is sometimes slow.
//! When a request is sent it will receive message for every packet + one connection closed packet.
//! This is why 101 and 2 are used instead of 100 and 1 for [`RECEIVED_MESSAGE_COUNT_HIGH`] and [`RECEIVED_MESSAGE_COUNT_LOW`].
use criterion::{criterion_group, criterion_main, Criterion};
use ilp_node::InterledgerNode;
use serde_json::{self, json};
use tokio::runtime::Runtime;
use tokio::sync::mpsc::channel;
use tungstenite::{client, handshake::client::Request};

mod redis_helpers;
mod test_helpers;

use redis_helpers::*;
use test_helpers::*;

const RECEIVED_MESSAGE_COUNT_HIGH: usize = 101;
const RECEIVED_MESSAGE_COUNT_LOW: usize = 2;

/// There will be executions where routes seem to be propagated right away during init, but on most
/// runs they require this propagation delay. It will create additional noise to results.
/// FIXME: this is a workaround to difficult startup
const ROUTE_BROADCAST_INTERVAL: u64 = 500;

fn multiple_payments_btp(c: &mut Criterion) {
    let mut rt = Runtime::new().unwrap();

    let node_a_http = get_open_port(None);
    let node_a_settlement = get_open_port(None);
    let node_b_http = get_open_port(None);
    let node_b_settlement = get_open_port(None);
    let context = TestContext::new();

    let mut connection_info1 = context.get_client_connection_info();
    connection_info1.redis.db = 1;
    let mut connection_info2 = context.get_client_connection_info();
    connection_info2.redis.db = 2;

    // accounts to be created on node a
    let alice_on_a = json!({
        "username": "alice_on_a",
        "asset_code": "XYZ",
        "asset_scale": 9,
        "ilp_over_http_incoming_token" : "default account holder",
        "max_packet_amount": 100,
    });
    let b_on_a = json!({
        "username": "b_on_a",
        "asset_code": "XYZ",
        "asset_scale": 9,
        "ilp_over_btp_url": format!("ws://localhost:{}/accounts/{}/ilp/btp", node_b_http, "a_on_b"),
        "ilp_over_btp_outgoing_token" : "token",
        "routing_relation": "Parent",
    });

    // accounts to be created on node b
    let a_on_b = json!({
        "username": "a_on_b",
        "asset_code": "XYZ",
        "asset_scale": 9,
        "ilp_over_btp_incoming_token" : "token",
        "routing_relation": "Child",
    });
    let bob_on_b = json!({
        "username": "bob_on_b",
        "asset_code": "XYZ",
        "asset_scale": 9,
        "ilp_over_http_incoming_token" : "default account holder",
    });

    // node a config
    let node_a: InterledgerNode = serde_json::from_value(json!({
        "admin_auth_token": "admin",
        "database_url": connection_info_to_string(connection_info1),
        "http_bind_address": format!("127.0.0.1:{}", node_a_http),
        "settlement_api_bind_address": format!("127.0.0.1:{}", node_a_settlement),
        "secret_seed": random_secret(),
        "route_broadcast_interval": ROUTE_BROADCAST_INTERVAL,
        "exchange_rate": {
            "poll_interval": 60000
        },
    }))
    .expect("Error creating node_a.");

    // node b config
    let node_b: InterledgerNode = serde_json::from_value(json!({
        "ilp_address": "example.parent",
        "default_spsp_account": "bob_on_b",
        "admin_auth_token": "admin",
        "database_url": connection_info_to_string(connection_info2),
        "http_bind_address": format!("127.0.0.1:{}", node_b_http),
        "settlement_api_bind_address": format!("127.0.0.1:{}", node_b_settlement),
        "secret_seed": random_secret(),
        "route_broadcast_interval": ROUTE_BROADCAST_INTERVAL,
        "exchange_rate": {
            "poll_interval": 60000
        },
    }))
    .expect("Error creating node_b.");

    rt.block_on(
        // start node b and open its accounts
        async {
            node_b.serve(None).await.unwrap();
            create_account_on_node(node_b_http, a_on_b, "admin")
                .await
                .unwrap();
            create_account_on_node(node_b_http, bob_on_b, "admin")
                .await
                .unwrap();

            // start node a and open its accounts
            node_a.serve(None).await.unwrap();
            create_account_on_node(node_a_http, alice_on_a, "admin")
                .await
                .unwrap();
            create_account_on_node(node_a_http, b_on_a, "admin")
                .await
                .unwrap();
        },
    );

    let ws_request = Request::builder()
        .uri(format!("ws://localhost:{}/payments/incoming", node_b_http))
        .header("Authorization", "Bearer admin")
        .body(())
        .unwrap();
    let (sender, mut receiver) = channel(RECEIVED_MESSAGE_COUNT_HIGH);
    let client = reqwest::Client::new();
    let req_low = client
        .post(&format!(
            "http://localhost:{}/accounts/{}/payments",
            node_a_http, "alice_on_a"
        ))
        .header(
            "Authorization",
            format!("Bearer {}", "default account holder"),
        )
        .json(&json!({
            "receiver": format!("http://localhost:{}/accounts/{}/spsp", node_b_http,"bob_on_b"),
            "source_amount": 100,
            "slippage": 0.025 // allow up to 2.5% slippage
        }));
    let req_high = client
        .post(&format!(
            "http://localhost:{}/accounts/{}/payments",
            node_a_http, "alice_on_a"
        ))
        .header(
            "Authorization",
            format!("Bearer {}", "default account holder"),
        )
        .json(&json!({
            "receiver": format!("http://localhost:{}/accounts/{}/spsp", node_b_http,"bob_on_b"),
            "source_amount": 10000,
            "slippage": 0.025 // allow up to 2.5% slippage
        }));
    let handle = std::thread::spawn(move || forward_payment_notifications(ws_request, sender));

    wait_for_propagation(&mut rt, &req_low, RECEIVED_MESSAGE_COUNT_LOW, &mut receiver);
    c.bench_function("process_payment_btp_single_packet", |b| {
        b.iter(|| bench_fn(&mut rt, &req_low, RECEIVED_MESSAGE_COUNT_LOW, &mut receiver));
    });
    c.bench_function("process_payment_btp_hundred_packets", |b| {
        b.iter(|| {
            bench_fn(
                &mut rt,
                &req_high,
                RECEIVED_MESSAGE_COUNT_HIGH,
                &mut receiver,
            )
        });
    });

    drop(rt);
    handle.join().unwrap().unwrap();
}
fn multiple_payments_http(c: &mut Criterion) {
    let mut rt = Runtime::new().unwrap();
    let node_a_http = get_open_port(None);
    let node_a_settlement = get_open_port(None);
    let node_b_http = get_open_port(None);
    let node_b_settlement = get_open_port(None);
    let context = TestContext::new();

    let mut connection_info1 = context.get_client_connection_info();
    connection_info1.redis.db = 1;
    let mut connection_info2 = context.get_client_connection_info();
    connection_info2.redis.db = 2;

    // accounts to be created on node a
    let alice_on_a = json!({
        "username": "alice_on_a",
        "asset_code": "XYZ",
        "asset_scale": 9,
        "ilp_over_http_incoming_token" : "admin",
        "max_packet_amount": 100,
    });
    let b_on_a = json!({
        "username": "b_on_a",
        "asset_code": "XYZ",
        "asset_scale": 9,
        "ilp_over_http_url": format!("http://localhost:{}/accounts/{}/ilp", node_b_http, "a_on_b"),
        "ilp_over_http_incoming_token" : "admin",
        "ilp_over_http_outgoing_token" : "admin",
        "ilp_address": "example.node_b",
    });

    // accounts to be created on node b
    let a_on_b = json!({
        "username": "a_on_b",
        "ilp_over_http_url": format!("http://localhost:{}/accounts/{}/ilp", node_a_http, "b_on_a"),
        "asset_code": "XYZ",
        "asset_scale": 9,
        "ilp_over_http_incoming_token" : "admin",
        "ilp_over_http_outgoing_token" : "admin",
    });
    let bob_on_b = json!({
        "username": "bob_on_b",
        "asset_code": "XYZ",
        "asset_scale": 9,
        "ilp_over_http_incoming_token" : "admin",
    });

    let node_a: InterledgerNode = serde_json::from_value(json!({
        "ilp_address": "example.node_a",
        "secret_seed" : random_secret(),
        "admin_auth_token": "admin",
        "database_url": connection_info_to_string(connection_info1),
        "http_bind_address": format!("127.0.0.1:{}", node_a_http),
        "settlement_api_bind_address": format!("127.0.0.1:{}", node_a_settlement),
        "route_broadcast_interval": ROUTE_BROADCAST_INTERVAL,
        "exchange_rate": {
            "poll_interval": 60000
        },
    }))
    .expect("Error creating node_a.");

    let node_b: InterledgerNode = serde_json::from_value(json!({
        "ilp_address": "example.node_b",
        "secret_seed" : random_secret(),
        "admin_auth_token": "admin",
        "database_url": connection_info_to_string(connection_info2),
        "http_bind_address": format!("127.0.0.1:{}", node_b_http),
        "settlement_api_bind_address": format!("127.0.0.1:{}", node_b_settlement),
        "default_spsp_account": "bob_on_b",
        "route_broadcast_interval": ROUTE_BROADCAST_INTERVAL,
        "exchange_rate": {
            "poll_interval": 60000
        },
    }))
    .expect("Error creating node_b.");
    rt.block_on(
        // start node b and open its accounts
        async {
            node_b.serve(None).await.unwrap();
            create_account_on_node(node_b_http, a_on_b, "admin")
                .await
                .unwrap();
            create_account_on_node(node_b_http, bob_on_b, "admin")
                .await
                .unwrap();

            // start node a and open its accounts
            node_a.serve(None).await.unwrap();
            create_account_on_node(node_a_http, alice_on_a, "admin")
                .await
                .unwrap();
            create_account_on_node(node_a_http, b_on_a, "admin")
                .await
                .unwrap();
        },
    );

    let ws_request = Request::builder()
        .uri(format!("ws://localhost:{}/payments/incoming", node_b_http))
        .header("Authorization", "Bearer admin")
        .body(())
        .unwrap();
    let (sender, mut receiver) = channel(RECEIVED_MESSAGE_COUNT_HIGH);
    let client = reqwest::Client::new();
    let req_low = client
        .post(&format!(
            "http://localhost:{}/accounts/{}/payments",
            node_a_http, "alice_on_a"
        ))
        .header("Authorization", format!("Bearer {}", "admin"))
        .json(&json!({
            "receiver": format!("http://localhost:{}/accounts/{}/spsp", node_b_http,"bob_on_b"),
            "source_amount": 100,
            "slippage": 0.025 // allow up to 2.5% slippage
        }));
    let req_high = client
        .post(&format!(
            "http://localhost:{}/accounts/{}/payments",
            node_a_http, "alice_on_a"
        ))
        .header("Authorization", format!("Bearer {}", "admin"))
        .json(&json!({
            "receiver": format!("http://localhost:{}/accounts/{}/spsp", node_b_http,"bob_on_b"),
            "source_amount": 10000,
            "slippage": 0.025 // allow up to 2.5% slippage
        }));
    let handle = std::thread::spawn(move || forward_payment_notifications(ws_request, sender));
    wait_for_propagation(&mut rt, &req_low, RECEIVED_MESSAGE_COUNT_LOW, &mut receiver);
    c.bench_function("process_payment_http_single_packet", |b| {
        b.iter(|| bench_fn(&mut rt, &req_low, RECEIVED_MESSAGE_COUNT_LOW, &mut receiver));
    });
    c.bench_function("process_payment_http_hundred_packets", |b| {
        b.iter(|| {
            bench_fn(
                &mut rt,
                &req_high,
                RECEIVED_MESSAGE_COUNT_HIGH,
                &mut receiver,
            )
        });
    });

    drop(rt);
    handle.join().unwrap().unwrap();
}

/// Forwards the payment notifications from the websocket into the channel
/// When runtime (rt) is dropped, error will happen ending the loop and then the websocket
fn forward_payment_notifications(
    ws_request: tungstenite::http::Request<()>,
    mut sender: tokio::sync::mpsc::Sender<tungstenite::Message>,
) -> Result<(), tungstenite::Error> {
    let mut payments_ws = client::connect(ws_request)?.0;
    while let Ok(message) = payments_ws.read_message() {
        sender.try_send(message).unwrap();
    }
    payments_ws.close(None)
}

/// Route propagation has a slow startup sometimes.
/// Loops until route propagation is on.
fn wait_for_propagation(
    rt: &mut tokio::runtime::Runtime,
    request: &reqwest::RequestBuilder,
    received_message_count: usize,
    receiver: &mut tokio::sync::mpsc::Receiver<tungstenite::Message>,
) {
    let delay_ms = 100;
    let timeout_ms = 40000;
    let tries_until_timeout = timeout_ms / delay_ms;
    rt.block_on(async {
        for i in 0..tries_until_timeout {
            let response = request.try_clone().unwrap().send().await.unwrap();
            if response.status().is_success() {
                break;
            }
            if i == tries_until_timeout - 1 {
                panic!("Timeout: Responses keep on failing: {:?}", response);
            }
            tokio::time::delay_for(std::time::Duration::from_millis(delay_ms)).await;
        }
        for _ in 0..received_message_count {
            // TODO check if received data is correct
            receiver.recv().await.unwrap();
        }
    });
}

fn bench_fn(
    rt: &mut tokio::runtime::Runtime,
    request: &reqwest::RequestBuilder,
    received_message_count: usize,
    receiver: &mut tokio::sync::mpsc::Receiver<tungstenite::Message>,
) {
    rt.block_on(async {
        let response = request.try_clone().unwrap().send().await.unwrap();
        if !&response.status().is_success() {
            panic!("Error receiving response: {:?}", response)
        }
        for _ in 0..received_message_count {
            // TODO check if received data is correct
            receiver.recv().await.unwrap();
        }
    });
}
criterion_group!(benches, multiple_payments_http, multiple_payments_btp);
criterion_main!(benches);
