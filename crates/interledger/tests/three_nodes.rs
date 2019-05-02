#![recursion_limit = "128"]

extern crate interledger;
extern crate reqwest;
#[macro_use]
extern crate serde_json;

use env_logger;
use futures::{future, Future};
use interledger::cli;
use tokio::runtime::Runtime;

mod redis_helpers;
use redis_helpers::*;

#[test]
fn three_nodes() {
    // Nodes 1 and 2 are peers, Node 2 is the parent of Node 3
    let _ = env_logger::try_init();
    let context = TestContext::new();

    // Each node will use its own DB within the redis instance
    let mut connection_info1 = context.get_client_connection_info();
    connection_info1.db = 1;
    let connection_info1_clone = connection_info1.clone();
    let mut connection_info2 = context.get_client_connection_info();
    connection_info2.db = 2;
    let connection_info2_clone = connection_info2.clone();
    let mut connection_info3 = context.get_client_connection_info();
    connection_info3.db = 3;
    let connection_info3_clone = connection_info3.clone();

    let server_secret1 = cli::random_secret();
    let server_secret2 = cli::random_secret();
    let server_secret3 = cli::random_secret();

    let node1_http = get_open_port(Some(3010));
    let node2_http = get_open_port(Some(3020));
    let node2_btp = get_open_port(Some(3021));
    let node3_http = get_open_port(Some(3030));

    let mut runtime = Runtime::new().unwrap();

    let node1 = future::join_all(vec![
        // Own account
        cli::insert_account_redis(
            connection_info1.clone(),
            &server_secret1,
            cli::AccountDetails {
                ilp_address: String::from("example.one"),
                asset_code: "XYZ".to_string(),
                asset_scale: 9,
                btp_incoming_token: None,
                btp_uri: None,
                http_endpoint: None,
                http_incoming_token: Some("admin".to_string()),
                http_outgoing_token: None,
                max_packet_amount: u64::max_value(),
                min_balance: i64::min_value(),
                is_admin: true,
                xrp_address: None,
                settle_threshold: None,
                settle_to: None,
                send_routes: false,
                receive_routes: false,
                routing_relation: None,
                round_trip_time: None,
                packets_per_minute_limit: None,
                amount_per_minute_limit: None,
            },
        ),
        cli::insert_account_redis(
            connection_info1.clone(),
            &server_secret1,
            cli::AccountDetails {
                ilp_address: String::from("example.two"),
                asset_code: "XYZ".to_string(),
                asset_scale: 9,
                btp_incoming_token: None,
                btp_uri: None,
                http_endpoint: Some(format!("http://localhost:{}/ilp", node2_http.clone())),
                http_incoming_token: Some("two".to_string()),
                http_outgoing_token: Some("one".to_string()),
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
        ),
    ])
    .and_then(move |_| {
        cli::run_node_redis(
            connection_info1_clone,
            ([127, 0, 0, 1], get_open_port(None)).into(),
            ([127, 0, 0, 1], node1_http).into(),
            &server_secret1,
        )
    });
    runtime.spawn(node1);

    let node2 = future::join_all(vec![
        // Own account
        cli::insert_account_redis(
            connection_info2.clone(),
            &server_secret2,
            cli::AccountDetails {
                ilp_address: String::from("example.two"),
                asset_code: "XYZ".to_string(),
                asset_scale: 9,
                btp_incoming_token: None,
                btp_uri: None,
                http_endpoint: None,
                http_incoming_token: Some("admin".to_string()),
                http_outgoing_token: None,
                max_packet_amount: u64::max_value(),
                min_balance: i64::min_value(),
                is_admin: true,
                xrp_address: None,
                settle_threshold: None,
                settle_to: None,
                send_routes: false,
                receive_routes: false,
                routing_relation: None,
                round_trip_time: None,
                packets_per_minute_limit: None,
                amount_per_minute_limit: None,
            },
        ),
        cli::insert_account_redis(
            connection_info2.clone(),
            &server_secret2,
            cli::AccountDetails {
                ilp_address: String::from("example.one"),
                asset_code: "XYZ".to_string(),
                asset_scale: 9,
                btp_incoming_token: None,
                btp_uri: None,
                http_endpoint: Some(format!("http://localhost:{}/ilp", node1_http.clone())),
                http_incoming_token: Some("one".to_string()),
                http_outgoing_token: Some("two".to_string()),
                max_packet_amount: u64::max_value(),
                min_balance: -1_000_000,
                is_admin: false,
                xrp_address: None,
                settle_threshold: None,
                settle_to: None,
                send_routes: true,
                receive_routes: true,
                routing_relation: Some("Peer".to_string()),
                round_trip_time: None,
                packets_per_minute_limit: None,
                amount_per_minute_limit: None,
            },
        ),
        cli::insert_account_redis(
            connection_info2.clone(),
            &server_secret2,
            cli::AccountDetails {
                ilp_address: String::from("example.two.three"),
                asset_code: "ABC".to_string(),
                asset_scale: 6,
                btp_incoming_token: Some("three".to_string()),
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
                send_routes: true,
                receive_routes: false,
                routing_relation: Some("Child".to_string()),
                round_trip_time: None,
                packets_per_minute_limit: None,
                amount_per_minute_limit: None,
            },
        ),
    ])
    .and_then(move |_| {
        cli::run_node_redis(
            connection_info2_clone,
            ([127, 0, 0, 1], node2_btp).into(),
            ([127, 0, 0, 1], node2_http).into(),
            &server_secret2,
        )
    })
    .and_then(move |_| {
        let client = reqwest::r#async::Client::new();
        client
            .put(&format!("http://localhost:{}/rates", node2_http))
            .header("Authorization", "Bearer admin")
            .json(&[("ABC", 1), ("XYZ", 2500)])
            .send()
            .map_err(|err| panic!(err))
            .and_then(|res| {
                res.error_for_status()
                    .expect("Error setting exchange rates");
                Ok(())
            })
    });
    runtime.spawn(node2);

    let node3 = future::join_all(vec![
        // Own account
        cli::insert_account_redis(
            connection_info3.clone(),
            &server_secret3,
            cli::AccountDetails {
                ilp_address: String::from("example.two.three"),
                asset_code: "ABC".to_string(),
                asset_scale: 6,
                btp_incoming_token: None,
                btp_uri: None,
                http_endpoint: None,
                http_incoming_token: Some("admin".to_string()),
                http_outgoing_token: None,
                max_packet_amount: u64::max_value(),
                min_balance: i64::min_value(),
                is_admin: true,
                xrp_address: None,
                settle_threshold: None,
                settle_to: None,
                send_routes: false,
                receive_routes: false,
                routing_relation: None,
                round_trip_time: None,
                packets_per_minute_limit: None,
                amount_per_minute_limit: None,
            },
        ),
        cli::insert_account_redis(
            connection_info3.clone(),
            &server_secret3,
            cli::AccountDetails {
                ilp_address: String::from("example.two"),
                asset_code: "ABC".to_string(),
                asset_scale: 6,
                btp_incoming_token: None,
                btp_uri: Some(format!("btp+ws://:three@localhost:{}", node2_btp)),
                http_endpoint: None,
                http_incoming_token: None,
                http_outgoing_token: None,
                max_packet_amount: u64::max_value(),
                min_balance: -1_000_000,
                is_admin: false,
                xrp_address: None,
                settle_threshold: None,
                settle_to: None,
                send_routes: true,
                receive_routes: true,
                routing_relation: Some("Parent".to_string()),
                round_trip_time: None,
                packets_per_minute_limit: None,
                amount_per_minute_limit: None,
            },
        ),
    ])
    .and_then(move |_| {
        cli::run_node_redis(
            connection_info3_clone,
            ([127, 0, 0, 1], get_open_port(None)).into(),
            ([127, 0, 0, 1], node3_http).into(),
            &server_secret3,
        )
    });
    runtime.spawn(node3);

    runtime
        .block_on(
            delay(1000)
                .map_err(|_| panic!("Something strange happened"))
                .and_then(move |_| {
                    let client = reqwest::r#async::Client::new();
                    let send_1_to_3 = client
                        .post(&format!("http://localhost:{}/pay", node1_http))
                        .header("Authorization", "Bearer admin")
                        .json(&json!({
                            "receiver": format!("http://localhost:{}/.well-known/pay", node3_http),
                            "source_amount": 1000,
                        }))
                        .send()
                        .and_then(|res| res.error_for_status());
                    // .and_then(|res| res.into_body().concat2())
                    // .and_then(|body| {
                    //     println!("{:?}", body);
                    //     Ok(())
                    // });

                    let send_3_to_1 = client
                        .post(&format!("http://localhost:{}/pay", node3_http))
                        .header("Authorization", "Bearer admin")
                        .json(&json!({
                                "receiver": format!("http://localhost:{}/.well-known/pay", node1_http).as_str(),
                            "source_amount": 1000,
                        }))
                        .send()
                        .and_then(|res| res.error_for_status());
                    // .and_then(|res| res.into_body().concat2())
                    // .and_then(|body| {
                    //     println!("{:?}", body);
                    //     Ok(())
                    // });

                    send_1_to_3
                        .map_err(|err| {
                            eprintln!("Error sending from node 1 to node 3: {:?}", err);
                            err
                        })
                        .and_then(|_| send_3_to_1)
                        .map_err(|err| {
                            eprintln!("Error sending from node 3 to node 1: {:?}", err);
                            err
                        })
                }),
        )
        .unwrap();
}
