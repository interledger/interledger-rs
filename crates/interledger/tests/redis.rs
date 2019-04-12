extern crate interledger;
#[macro_use]
extern crate log;

use env_logger;
use futures::{future::ok, Future};
use interledger::cli;
use std::time::{Duration, Instant};
use tokio::{runtime::Runtime, timer::Delay};

mod redis_helpers;
use redis_helpers::*;

fn get_open_port(try_port: Option<u16>) -> u16 {
    if let Some(port) = try_port {
        let listener = net2::TcpBuilder::new_v4().unwrap();
        listener.reuse_address(true).unwrap();
        if let Ok(listener) = listener.bind(&format!("127.0.0.1:{}", port)) {
            return listener.listen(1).unwrap().local_addr().unwrap().port();
        }
    }

    for _i in 0..1000 {
        let listener = net2::TcpBuilder::new_v4().unwrap();
        listener.reuse_address(true).unwrap();
        if let Ok(listener) = listener.bind("127.0.0.1:0") {
            return listener.listen(1).unwrap().local_addr().unwrap().port();
        }
    }
    panic!("Cannot find open port!");
}

fn delay(ms: u64) -> impl Future<Item = (), Error = ()> {
    Delay::new(Instant::now() + Duration::from_millis(ms)).map_err(|err| panic!(err))
}

#[test]
fn btp_end_to_end() {
    let _ = env_logger::try_init();
    let context = TestContext::new();
    let connection_info1 = context.get_client_connection_info();
    let connection_info2 = context.get_client_connection_info();
    let connection_info3 = context.get_client_connection_info();
    let server_secret = cli::random_secret();
    // let redis_port = 6379;
    let btp_port = get_open_port(Some(7768));
    let http_port = get_open_port(Some(7770));
    let run = ok(()).and_then(move |_| {
        let create_accounts = cli::insert_account_redis(
            connection_info1,
            &server_secret,
            cli::AccountDetails {
                ilp_address: String::from("example.one"),
                asset_code: "XYZ".to_string(),
                asset_scale: 9,
                btp_incoming_token: Some("token-one".to_string()),
                btp_uri: None,
                http_endpoint: None,
                http_incoming_token: None,
                http_outgoing_token: None,
                max_packet_amount: u64::max_value(),
                min_balance: -1_000_000,
                is_admin: false,
                xrp_address: None,
                settle_threshold: None,
                settle_to: None,
                send_routes: false,
                receive_routes: false,
                routing_relation: Some("Peer".to_string()),
                round_trip_time: None,
                packets_per_minute_limit: None,
                amount_per_minute_limit: None,
            },
        )
        .and_then(move |_| {
            cli::insert_account_redis(
                connection_info2,
                &server_secret,
                cli::AccountDetails {
                    ilp_address: String::from("example.two"),
                    asset_code: "XYZ".to_string(),
                    asset_scale: 9,
                    btp_incoming_token: Some("token-two".to_string()),
                    btp_uri: None,
                    http_endpoint: None,
                    http_incoming_token: None,
                    http_outgoing_token: None,
                    max_packet_amount: u64::max_value(),
                    min_balance: -1_000_000,
                    is_admin: false,
                    xrp_address: None,
                    settle_threshold: None,
                    settle_to: None,
                    send_routes: false,
                    receive_routes: false,
                    routing_relation: Some("Peer".to_string()),
                    round_trip_time: None,
                    packets_per_minute_limit: None,
                    amount_per_minute_limit: None,
                },
            )
        });

        let spawn_connector = move |_| {
            debug!("Spawning connector");
            // Note: this needs to be run AFTER the accounts are created because the
            // store does not currently subscribe to notifications of accounts being created
            // or the routing table being updated
            let connector = interledger::cli::run_node_redis(
                connection_info3,
                ([127, 0, 0, 1], btp_port).into(),
                ([127, 0, 0, 1], http_port).into(),
                &server_secret,
                None,
            );
            tokio::spawn(connector);
            Ok(())
        };

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

        create_accounts
            .and_then(spawn_connector)
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
