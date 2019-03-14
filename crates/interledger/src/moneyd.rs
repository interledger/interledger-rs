use bytes::Bytes;
use futures::Future;
use interledger_btp::create_open_signup_server;
use interledger_ildcp::{IldcpResponse, IldcpService};
use interledger_packet::{ErrorCode, RejectBuilder};
use interledger_router::Router;
use interledger_service::outgoing_service_fn;
use interledger_service_util::ValidatorService;
use interledger_store_memory::InMemoryStore;
use std::net::SocketAddr;
use tokio;

#[doc(hidden)]
pub fn run_moneyd_local(address: SocketAddr, ildcp_info: IldcpResponse) {
    let ilp_address = Bytes::from(ildcp_info.client_address());
    let store = InMemoryStore::default();
    // TODO this needs a reference to the BtpService so it can send outgoing packets
    println!("Listening on: {}", address);
    let ilp_address_clone = ilp_address.clone();
    let rejecter = outgoing_service_fn(move |_| {
        Err(RejectBuilder {
            code: ErrorCode::F02_UNREACHABLE,
            message: b"No open connection for account",
            triggered_by: &ilp_address_clone[..],
            data: &[],
        }
        .build())
    });
    let server = create_open_signup_server(address, ildcp_info, store.clone(), rejecter).and_then(
        move |btp_service| {
            let service = Router::new(btp_service.clone(), store);
            let service = IldcpService::new(service);
            let service = ValidatorService::incoming(service);
            btp_service.handle_incoming(service);
            Ok(())
        },
    );
    tokio::run(server);
}
