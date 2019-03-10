use env_logger;
use futures::Future;
use interledger_ildcp::{IldcpResponseBuilder, IldcpService};
use interledger_service_util::RejecterService;
use interledger_stream::{send_money, ConnectionGenerator, StreamReceiverService};
use interledger_test_helpers::TestAccount;
use tokio::runtime::Runtime;

#[test]
fn send_money_test() {
    env_logger::init();
    let server_secret = [0; 32];
    let destination_address = b"example.receiver";
    let ildcp_info = IldcpResponseBuilder {
        client_address: destination_address,
        asset_code: "XYZ",
        asset_scale: 9,
    }
    .build();
    let connection_generator = ConnectionGenerator::new(destination_address, &server_secret[..]);
    let server = StreamReceiverService::new(
        &server_secret,
        ildcp_info,
        RejecterService::new(destination_address),
    );
    let server = IldcpService::new(server);

    let (destination_account, shared_secret) =
        connection_generator.generate_address_and_secret(b"test");

    let run = send_money(
        server,
        &TestAccount::default(),
        &destination_account[..],
        &shared_secret[..],
        100,
    )
    .and_then(|(amount_delivered, _service)| {
        assert_eq!(amount_delivered, 100);
        Ok(())
    })
    .map_err(|err| panic!(err));
    let runtime = Runtime::new().unwrap();
    runtime.block_on_all(run).unwrap();
}
