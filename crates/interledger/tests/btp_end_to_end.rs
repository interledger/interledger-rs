use env_logger;
use futures::{
    future::{join_all, ok},
    Future,
};
use interledger::{
    cli,
    node::{AccountDetails, InterledgerNode},
};
use interledger_packet::Address;
use log::debug;
use std::str::FromStr;
use tokio::runtime::Runtime;

mod redis_helpers;
use redis_helpers::*;

#[test]
fn btp_end_to_end() {
    let _ = env_logger::try_init();
    let context = TestContext::new();
    let btp_port = get_open_port(Some(7768));
    let http_port = get_open_port(Some(7770));
    let settlement_port = get_open_port(Some(7771));
    let node = InterledgerNode {
        ilp_address: Address::from_str("example.node").unwrap(),
        default_spsp_account: None,
        admin_auth_token: "admin".to_string(),
        redis_connection: context.get_client_connection_info(),
        btp_address: ([127, 0, 0, 1], btp_port).into(),
        http_address: ([127, 0, 0, 1], http_port).into(),
        settlement_address: ([127, 0, 0, 1], settlement_port).into(),
        secret_seed: cli::random_secret(),
        route_broadcast_interval: Some(200),
    };
    let run = ok(()).and_then(move |_| {
        let spawn_connector = ok(tokio::spawn(node.serve())).and_then(move |_| {
            join_all(vec![
                node.insert_account(AccountDetails {
                    ilp_address: Address::from_str("example.node.one").unwrap(),
                    asset_code: "XYZ".to_string(),
                    asset_scale: 9,
                    btp_incoming_token: Some("token-one".to_string()),
                    btp_uri: None,
                    http_endpoint: None,
                    http_incoming_token: None,
                    http_outgoing_token: None,
                    max_packet_amount: u64::max_value(),
                    min_balance: None,
                    settle_threshold: None,
                    settle_to: None,
                    send_routes: false,
                    receive_routes: false,
                    routing_relation: None,
                    round_trip_time: None,
                    packets_per_minute_limit: None,
                    amount_per_minute_limit: None,
                    settlement_engine_url: None,
                }),
                node.insert_account(AccountDetails {
                    ilp_address: Address::from_str("example.node.two").unwrap(),
                    asset_code: "XYZ".to_string(),
                    asset_scale: 9,
                    btp_incoming_token: Some("token-two".to_string()),
                    btp_uri: None,
                    http_endpoint: None,
                    http_incoming_token: None,
                    http_outgoing_token: None,
                    max_packet_amount: u64::max_value(),
                    min_balance: None,
                    settle_threshold: None,
                    settle_to: None,
                    send_routes: false,
                    receive_routes: false,
                    routing_relation: None,
                    round_trip_time: None,
                    packets_per_minute_limit: None,
                    amount_per_minute_limit: None,
                    settlement_engine_url: None,
                }),
            ])
        });

        let spsp_server_port = get_open_port(Some(3000));
        let spawn_spsp_server = move |_| {
            debug!("Spawning SPSP server");
            let spsp_server = cli::run_spsp_server_btp(
                &format!("btp+ws://:token-one@localhost:{}", btp_port),
                ([127, 0, 0, 1], spsp_server_port).into(),
                true,
            );
            tokio::spawn(spsp_server);
            Ok(())
        };

        spawn_connector
            .and_then(|_| delay(200))
            .and_then(spawn_spsp_server)
            .and_then(|_| delay(200))
            .and_then(move |_| {
                cli::send_spsp_payment_btp(
                    &format!("btp+ws://:token-two@localhost:{}", btp_port),
                    &format!("http://localhost:{}", spsp_server_port),
                    10000,
                    true,
                )
                .then(move |result| {
                    let _ = context;
                    result
                })
            })
    });
    let mut runtime = Runtime::new().unwrap();
    runtime.block_on(run).unwrap();
}
