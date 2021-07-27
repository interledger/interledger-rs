use crate::redis_helpers::*;
use crate::test_helpers::*;
use ilp_node::InterledgerNode;
use serde::Deserialize;
use serde_json::{self, json};
use tokio_stream::StreamExt;
use tungstenite::{client, handshake::client::Request};

#[tokio::test]
async fn payments_incoming() {
    let context = TestContext::new();

    let mut connection_info1 = context.get_client_connection_info();
    connection_info1.redis.db = 1;

    // test ports
    let node_a_http = get_open_port(None);
    let node_a_settlement = get_open_port(None);
    let node_b_http = get_open_port(None);
    let node_b_settlement = get_open_port(None);

    // accounts to be created on node a
    let alice_on_a = json!({
        "username": "alice_on_a",
        "asset_code": "XYZ",
        "asset_scale": 9,
        "ilp_over_http_incoming_token" : "default account holder",
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
    let caleb_on_b = json!({
        "username": "caleb_on_b",
        "asset_code": "XYZ",
        "asset_scale": 9,
        "ilp_over_http_incoming_token" : "default account holder",
    });
    let dave_on_b = json!({
        "username": "dave_on_b",
        "asset_code": "XYZ",
        "asset_scale": 9,
        "ilp_over_http_incoming_token" : "default account holder",
    });

    // node a config
    let node_a: InterledgerNode = serde_json::from_value(json!({
        "admin_auth_token": "admin",
        "database_url": connection_info_to_string(connection_info1.clone()),
        "database_prefix": "testnodea",
        "http_bind_address": format!("127.0.0.1:{}", node_a_http),
        "settlement_api_bind_address": format!("127.0.0.1:{}", node_a_settlement),
        "secret_seed": random_secret(),
        "route_broadcast_interval": 200,
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
        "database_url": connection_info_to_string(connection_info1),
        "database_prefix": "testnodeb",
        "http_bind_address": format!("127.0.0.1:{}", node_b_http),
        "settlement_api_bind_address": format!("127.0.0.1:{}", node_b_settlement),
        "secret_seed": random_secret(),
        "route_broadcast_interval": Some(200),
        "exchange_rate": {
            "poll_interval": 60000
        },
    }))
    .expect("Error creating node_b.");

    // start node b and open its accounts
    node_b.serve(None).await.unwrap();
    create_account_on_node(node_b_http, a_on_b, "admin")
        .await
        .unwrap();
    create_account_on_node(node_b_http, bob_on_b, "admin")
        .await
        .unwrap();
    create_account_on_node(node_b_http, caleb_on_b, "admin")
        .await
        .unwrap();
    create_account_on_node(node_b_http, dave_on_b, "admin")
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

    #[allow(non_snake_case)]
    #[derive(Deserialize)]
    struct PmtNotificationWrapper {
        Ok: PmtNotification,
    }

    #[derive(Deserialize)]
    struct PmtNotification {
        to_username: String,
        from_username: String,
        amount: u64,
    }

    // create a cross-thread collection of payment notifications
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    let mut rx = tokio_stream::wrappers::UnboundedReceiverStream::new(rx);
    let handle = std::thread::spawn(move || {
        let ws_request = Request::builder()
            .uri(format!("ws://localhost:{}/payments/incoming", node_b_http))
            .header("Authorization", "Bearer admin")
            .body(())
            .unwrap();

        let mut payments_ws = client::connect(ws_request).unwrap().0;

        // loop as many times as there are expected payment notifications
        for _ in 0..3 {
            let msg = payments_ws.read_message().unwrap();
            let payment: PmtNotificationWrapper =
                serde_json::from_str(&msg.into_text().unwrap()).unwrap();
            let msg = payments_ws.read_message().unwrap();
            let close: PmtNotificationWrapper =
                serde_json::from_str(&msg.into_text().unwrap()).unwrap();
            tx.send((payment.Ok, close.Ok))
                .map_err(|_| ())
                .expect("send failed");
        }

        payments_ws.close(None).unwrap();
    });

    // send money from alice (node a) to bob (node b)
    send_money_to_username(
        node_a_http,
        node_b_http,
        1000,
        "bob_on_b",
        "alice_on_a",
        "default account holder",
    )
    .await
    .unwrap();

    // send money from alice (node a) to caleb (node b)
    send_money_to_username(
        node_a_http,
        node_b_http,
        2000,
        "caleb_on_b",
        "alice_on_a",
        "default account holder",
    )
    .await
    .unwrap();

    // send money from alice (node a) to dave (node b)
    send_money_to_username(
        node_a_http,
        node_b_http,
        3000,
        "dave_on_b",
        "alice_on_a",
        "default account holder",
    )
    .await
    .unwrap();

    // FIXME: this should be doable with rx.collect::<????>().await.unwrap()
    let mut messages = vec![];
    loop {
        let next = rx.next().await;
        if let Some(next) = next {
            messages.push(next);
        } else {
            break;
        }
    }

    handle
        .join()
        .expect("as the messages stopped the thread should have exited as well");

    // check if all the payment notifications were received as expected
    assert_eq!(messages.len(), 3);

    assert_eq!(messages[0].0.to_username, "bob_on_b");
    assert_eq!(messages[1].0.to_username, "caleb_on_b");
    assert_eq!(messages[2].0.to_username, "dave_on_b");

    assert_eq!(messages[0].0.from_username, "a_on_b");
    assert_eq!(messages[1].0.from_username, "a_on_b");
    assert_eq!(messages[2].0.from_username, "a_on_b");

    assert_eq!(messages[0].0.amount, 1000);
    assert_eq!(messages[1].0.amount, 2000);
    assert_eq!(messages[2].0.amount, 3000);
}
