use crate::redis_helpers::{connection_info_to_string, get_open_port, TestContext};
use crate::test_helpers::{create_account_on_node, random_secret};
use ilp_node::InterledgerNode;
use serde_json::json;
use std::convert::TryFrom;

#[tokio::test]
#[cfg(feature = "balance-tracking")]
async fn time_based_settlement() {
    // Create two nodes, two payments:
    //
    // 1. over settlement threshold
    // 2. wait for 1/4 of configured settle_every to get the settlement
    // 3. below settlement threshold
    // 4. expect to get the settlement in 3/4..5/4 of configured settle_every
    //
    // This was created following example of payments_incoming. The routing could be different.

    let settle_threshold = 500;
    let settle_to = 0;
    let settle_delay = std::time::Duration::from_secs(1);

    let context = TestContext::new();

    let mut node_a_connections = context.get_client_connection_info();
    node_a_connections.redis.db = 1;
    let mut node_b_connections = context.get_client_connection_info();
    node_b_connections.redis.db = 2;

    let node_a_http = get_open_port(None);
    let node_a_settlement = get_open_port(None);
    let node_b_http = get_open_port(None);
    let node_b_settlement = get_open_port(None);

    // Launch a in-test settlement engine which will communicate the API back to us through the
    // channel. Shutdown channel might not be necessary.
    let (_shutdown, shutdown_rx) = tokio::sync::oneshot::channel::<()>();
    let (tx, mut rx) = tokio::sync::mpsc::channel(1);
    let (node_b_settlement_engine, settlement_engine_server) = warp::serve(settlement_engine(tx))
        .bind_with_graceful_shutdown(([127, 0, 0, 1], 0), async move {
            // not sure if this is required
            let _ = shutdown_rx.await;
        });

    tokio::task::spawn(settlement_engine_server);

    let asset_code = "XYZ";
    let asset_scale: usize = 9;
    let http_secret = "default account holder";
    let btp_secret = "token";

    // accounts to be created on node a
    let alice_on_a = json!({
        "username": "alice_on_a",
        "asset_code": asset_code,
        "asset_scale": asset_scale,
        "ilp_over_http_incoming_token": http_secret,
    });
    let b_on_a = json!({
        "username": "b_on_a",
        "asset_code": asset_code,
        "asset_scale": asset_scale,
        "ilp_over_btp_url": format!("ws://localhost:{}/accounts/{}/ilp/btp", node_b_http, "a_on_b"),
        "ilp_over_btp_outgoing_token" : btp_secret,
        "routing_relation": "Parent",
        "settlement_engine_url": format!("http://{}:{}", node_b_settlement_engine.ip(), node_b_settlement_engine.port()),
    });

    // accounts to be created on node b
    let a_on_b = json!({
        "username": "a_on_b",
        "asset_code": asset_code,
        "asset_scale": asset_scale,
        "ilp_over_btp_incoming_token" : btp_secret,
        "routing_relation": "Child",
        "prepaid_balance": "0",
        "settle_threshold": settle_threshold,
        "settle_to": settle_to,
    });
    let bob_on_b = json!({
        "username": "bob_on_b",
        "asset_code": asset_code,
        "asset_scale": asset_scale,
        "ilp_over_http_incoming_token" : http_secret,
        "prepaid_balance": "0",
        "settle_threshold": settle_threshold,
        "settle_to": settle_to,
        "settlement_engine_url": format!("http://{}:{}", node_b_settlement_engine.ip(), node_b_settlement_engine.port()),
    });

    // node a config
    let node_a: InterledgerNode = serde_json::from_value(json!({
        "admin_auth_token": "admin",
        "database_url": connection_info_to_string(node_a_connections),
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
        "database_url": connection_info_to_string(node_b_connections),
        "http_bind_address": format!("127.0.0.1:{}", node_b_http),
        "settlement_api_bind_address": format!("127.0.0.1:{}", node_b_settlement),
        "secret_seed": random_secret(),
        "route_broadcast_interval": 200,
        "exchange_rate": {
            "poll_interval": 60000
        },
        // different from the other cases
        "settle_every": settle_delay.as_secs(),
        "settlement_engine_url": format!("http://{}:{}", node_b_settlement_engine.ip(), node_b_settlement_engine.port()),
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

    let bob_on_b_id = match rx.recv().await.unwrap() {
        SettlementEngineEvent::AccountCreation(id) => id,
        x => unreachable!("{:?}", x),
    };

    // start node a and open its accounts
    node_a.serve(None).await.unwrap();
    create_account_on_node(node_a_http, alice_on_a, "admin")
        .await
        .unwrap();
    create_account_on_node(node_a_http, b_on_a, "admin")
        .await
        .unwrap();

    let _b_on_a_id = match rx.recv().await.unwrap() {
        SettlementEngineEvent::AccountCreation(id) => id,
        x => unreachable!("{:?}", x),
    };

    // no need to listen to notifications
    crate::test_helpers::send_money_to_username(
        node_a_http,
        node_b_http,
        settle_threshold + 1,
        "bob_on_b",
        "alice_on_a",
        http_secret,
    )
    .await
    .unwrap();

    match tokio::time::timeout(settle_delay / 8, rx.recv()).await {
        Ok(Some(SettlementEngineEvent::Settlement {
            account,
            amount,
            scale,
        })) => {
            assert_eq!(account, bob_on_b_id);
            assert_eq!(amount, settle_threshold + 1);
            assert_eq!(scale, 9);
        }
        x => unreachable!("{:?}", x),
    };

    let before = std::time::Instant::now();

    crate::test_helpers::send_money_to_username(
        node_a_http,
        node_b_http,
        settle_threshold - 1,
        "bob_on_b",
        "alice_on_a",
        http_secret,
    )
    .await
    .unwrap();

    match tokio::time::timeout(settle_delay * 2, rx.recv()).await {
        Ok(Some(SettlementEngineEvent::Settlement {
            account,
            amount,
            scale,
        })) => {
            assert_eq!(account, bob_on_b_id);
            assert_eq!(amount, settle_threshold - 1);
            assert_eq!(scale, 9);
        }
        x => unreachable!("{:?}", x),
    };

    let elapsed = before.elapsed();

    let expected = i64::try_from(settle_delay.as_millis()).unwrap();
    let relative = i64::try_from(elapsed.as_millis()).unwrap() - expected;

    assert!(
        relative.abs() < (expected / 4),
        "settlement took unexpected amount of time: {:?}",
        elapsed
    );
}

#[derive(Debug)]
enum SettlementEngineEvent {
    AccountCreation(uuid::Uuid),
    Settlement {
        account: uuid::Uuid,
        amount: u64,
        scale: usize,
    },
    UnexpectedRequest(String),
}

/// Minimal settlement engine API for the test purposes.
fn settlement_engine(
    tx: tokio::sync::mpsc::Sender<SettlementEngineEvent>,
) -> impl warp::Filter<Extract = (impl warp::Reply,), Error = warp::Rejection> + Clone {
    use uuid::Uuid;
    use warp::Filter;

    // need to catch POST /accounts b"{\"id\":\"58f9c44b-f47b-4f2b-84eb-5822ba3996f5\"}"
    // and POST accounts/58f9c44b-f47b-4f2b-84eb-5822ba3996f5/settlements: b"{\"amount\":\"1000\",\"scale\":9}"

    #[derive(serde::Deserialize)]
    struct AccountInfo {
        id: Uuid,
    }

    #[derive(serde::Deserialize)]
    struct SettlementInfo {
        amount: String,
        scale: usize,
    }

    #[derive(Debug)]
    struct InternalError;

    impl warp::reject::Reject for InternalError {}

    let create_account = {
        let tx = tx.clone();
        warp::post()
            .and(warp::path!("accounts"))
            .and(warp::body::json())
            .and_then(move |body: AccountInfo| {
                let mut tx = tx.clone();
                async move {
                    tracing::trace!("begin sending");
                    tx.send(SettlementEngineEvent::AccountCreation(body.id))
                        .await
                        .map_err(|_| warp::reject::custom(InternalError))?;
                    tracing::trace!("after sending");
                    Ok::<_, warp::Rejection>((warp::reply(),))
                }
            })
    };

    let create_settlement = {
        let tx = tx.clone();

        warp::post()
            .and(warp::path!("accounts" / Uuid / "settlements"))
            .and(warp::body::json())
            .and_then(move |id: Uuid, body: SettlementInfo| {
                let mut tx = tx.clone();
                let amount = body.amount.parse::<u64>().unwrap();
                let scale = body.scale;
                async move {
                    tx.send(SettlementEngineEvent::Settlement {
                        account: id,
                        amount,
                        scale,
                    })
                    .await
                    .map_err(|_| warp::reject::custom(InternalError))?;
                    Ok::<_, warp::Rejection>((warp::reply(),))
                }
            })
    };

    let others = warp::any()
        .and(warp::path::peek())
        .and(warp::body::bytes())
        .and_then(move |p: warp::path::Peek, body| {
            let mut tx = tx.clone();
            tracing::warn!("{}: {:02x?}", p.as_str(), body);
            async move {
                tx.send(SettlementEngineEvent::UnexpectedRequest(
                    p.as_str().to_owned(),
                ))
                .await
                .map_err(|_| warp::reject::custom(InternalError))?;

                Ok::<_, warp::Rejection>((warp::reply(),))
            }
        });

    let recover = |err: warp::Rejection| {
        use warp::hyper::http::StatusCode;
        let code;
        let message;

        if err.is_not_found() {
            code = StatusCode::NOT_FOUND;
            message = "Not found";
        } else {
            code = StatusCode::INTERNAL_SERVER_ERROR;
            message = "Internal server error";
        }

        futures::future::ready(Ok::<_, _>(warp::reply::with_status(message, code)))
    };

    warp::any()
        .and(create_account.or(create_settlement).or(others))
        .recover(recover)
        .with(warp::trace::named("fake_settlement_engine"))
}
