use bytes::Bytes;
use futures::Future;
use interledger_btp::{connect_client, parse_btp_url};
use interledger_router::Router;
use interledger_service_util::RejecterService;
use interledger_spsp::pay;
use interledger_store_memory::{Account, InMemoryStore};
use std::u64;
use tokio;

const ACCOUNT_ID: u64 = 0;

pub fn send_spsp_payment(btp_server: &str, receiver: &str, amount: u64, quiet: bool) {
    let receiver = receiver.to_string();
    let store = InMemoryStore::new(vec![(
        ACCOUNT_ID,
        Account {
            ilp_address: Bytes::new(),
            // Send everything to this account
            additional_routes: vec![Bytes::from("".as_bytes())],
            asset_code: String::new(),
            asset_scale: 0,
            http_endpoint: None,
            http_incoming_authorization: None,
            http_outgoing_authorization: None,
            btp_url: Some(parse_btp_url(btp_server).unwrap()),
            btp_incoming_authorization: None,
            max_packet_amount: u64::max_value(),
        },
    )]);
    let run = connect_client(RejecterService::default(), store.clone(), vec![ACCOUNT_ID])
        .map_err(|err| {
            eprintln!("Error connecting to BTP server: {:?}", err);
            eprintln!("(Hint: is moneyd running?)");
        })
        .and_then(move |service| {
            let router = Router::new(service, store);
            pay(router, ACCOUNT_ID, &receiver, amount)
                .map_err(|err| {
                    eprintln!("Error sending SPSP payment: {:?}", err);
                })
                .and_then(move |delivered| {
                    if !quiet {
                        println!(
                            "Sent: {}, delivered: {} (in the receiver's units)",
                            amount, delivered
                        );
                    }
                    Ok(())
                })
        });
    tokio::run(run);
}

// fn run_spsp_server(
//     btp_server: &str,
//     port: u16,
//     notification_endpoint: Option<String>,
//     quiet: bool,
// ) {
//     let notification_endpoint = Arc::new(notification_endpoint);

//     // TODO make sure that the client keeps the connections alive
//     let client = Arc::new(reqwest::async::Client::new());

//     let run = btp::connect_async(&btp_server)
//     .map_err(|err| {
//       eprintln!("Error connecting to BTP server: {:?}", err);
//       eprintln!("(Hint: is moneyd running?)");
//     })
//     .and_then(move |plugin| {
//       spsp::listen_with_random_secret(plugin, port)
//         .map_err(|err| {
//           eprintln!("Error listening: {}", err);
//         })
//         .and_then(move |listener| {
//           let handle_connections = listener.for_each(move |(id, connection)| {
//             // TODO should the STREAM or SPSP server automatically remove this?
//             let split: Vec<&str> = id.splitn(2, '~').collect();

//             let client = Arc::clone(&client);
//             // TODO close the connection if it doesn't have a tag?
//             let conn_id = Arc::new(split[1].to_string());
//             let conn_id_clone = Arc::clone(&conn_id);

//             let notification_endpoint = Arc::clone(&notification_endpoint);
//             let handle_streams = connection.for_each(move |stream| {
//               let notification_endpoint = Arc::clone(&notification_endpoint);
//               let client = Arc::clone(&client);
//               let conn_id = Arc::clone(&conn_id);

//               let handle_money = stream.money.for_each(move |amount| {
//                 if let Some(ref url) = *notification_endpoint {
//                   let conn_id = Arc::clone(&conn_id);
//                   let body = json!({
//                     "receiver": *conn_id,
//                     "amount": amount,
//                   }).to_string();
//                   let send_notification = client.post(url)
//                     .header("Content-Type", "application/json")
//                     .body(body)
//                     .send()
//                     .map_err(move |err| {
//                       eprintln!("Error sending notification (got incoming money: {} for receiver: {}): {:?}", amount, conn_id, err);
//                     })
//                     .and_then(|_| {
//                       Ok(())
//                     });
//                   tokio::spawn(send_notification);
//                 } else if !quiet {
//                   println!("{}: Received: {} for connection: {}", Utc::now().to_rfc3339(), amount, conn_id);
//                 }

//                 Ok(())
//               });
//               tokio::spawn(handle_money);
//               Ok(())
//             })
//             .and_then(move |_| {
//               if !quiet {
//                 println!("{}: Connection closed: {}", Utc::now().to_rfc3339(), conn_id_clone);
//               }
//               Ok(())
//             });
//             tokio::spawn(handle_streams);
//             Ok(())
//           });
//           tokio::spawn(handle_connections);
//           if !quiet {
//             println!("Listening for SPSP connections on port: {}", port);
//           }
//           Ok(())
//         })
//     });
//     tokio::run(run);
// }
