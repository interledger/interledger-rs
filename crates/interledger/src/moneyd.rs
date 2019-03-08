use bytes::Bytes;
use futures::Future;
use interledger_btp::create_open_signup_server;
use interledger_ildcp::{IldcpResponse, IldcpService};
use interledger_router::Router;
use interledger_service_util::{RejecterService, ValidatorService};
use interledger_store_memory::InMemoryStore;
use std::net::SocketAddr;
use tokio;

pub fn run_moneyd_local(address: SocketAddr, ildcp_info: IldcpResponse) {
    let ilp_address = Bytes::from(ildcp_info.client_address());
    let store = InMemoryStore::default();
    // TODO this needs a reference to the BtpService so it can send outgoing packets
    println!("Listening on: {}", address);
    let server = create_open_signup_server(
        address,
        ildcp_info,
        store.clone(),
        RejecterService::new(&ilp_address[..]),
    )
    .and_then(move |btp_service| {
        let service = Router::new(btp_service.clone(), store);
        let service = IldcpService::new(service);
        let service = ValidatorService::incoming(service);
        btp_service.handle_incoming(service);
        Ok(())
    });
    tokio::run(server);
}
