use bytes::Bytes;
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
    let incoming_handler = Router::new(RejecterService::new(&ilp_address[..]), store.clone());
    let incoming_handler = IldcpService::new(incoming_handler);
    let incoming_handler = ValidatorService::incoming(incoming_handler);
    println!("Listening on: {}", address);
    let server = create_open_signup_server(
        address,
        ildcp_info,
        store,
        incoming_handler,
        RejecterService::new(&ilp_address[..]),
    );
    tokio::run(server);
}
