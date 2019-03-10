use futures::Future;
use interledger_btp::*;
use interledger_packet::{FulfillBuilder, PrepareBuilder};
use interledger_service::{OutgoingRequest, OutgoingService};
use interledger_service_util::RejecterService;
use interledger_store_memory::{AccountBuilder, InMemoryStore};
use interledger_test_helpers::TestIncomingService;
use std::time::{Duration, SystemTime};
use tokio::runtime::Runtime;
use url::Url;

#[test]
fn client_server_test() {
    env_logger::init();
    let mut runtime = Runtime::new().unwrap();

    let server_store = InMemoryStore::from_accounts(vec![AccountBuilder::new()
        .btp_incoming_token("test_auth_token".to_string())
        .build()]);
    let server = create_server(
        "127.0.0.1:12345".parse().unwrap(),
        server_store,
        RejecterService::default(),
    )
    .and_then(|btp_server| {
        btp_server.handle_incoming(TestIncomingService::fulfill(
            FulfillBuilder {
                fulfillment: &[0; 32],
                data: b"test data",
            }
            .build(),
        ));
        Ok(())
    });
    runtime.spawn(server);

    let account = AccountBuilder::new()
        .btp_uri(Url::parse("btp+ws://:test_auth_token@127.0.0.1:12345").unwrap())
        .build();
    let client_store = InMemoryStore::from_accounts(vec![account.clone()]);
    let accounts: Vec<u64> = vec![0];
    let client = connect_client(
        RejecterService::default(),
        RejecterService::default(),
        client_store,
        accounts,
    )
    .and_then(move |mut btp_service| {
        btp_service
            .send_request(OutgoingRequest {
                from: account.clone(),
                to: account.clone(),
                prepare: PrepareBuilder {
                    destination: b"example.destination",
                    amount: 100,
                    execution_condition: &[0; 32],
                    expires_at: SystemTime::now() + Duration::from_secs(30),
                    data: b"test data",
                }
                .build(),
            })
            .map_err(|reject| println!("Packet was rejected: {:?}", reject))
            .and_then(|_| Ok(()))
    });
    runtime.block_on_all(client).unwrap();
}
