use base64;
use bytes::Bytes;
use futures::{future::ok, Future};
use hyper::{
    header::{HeaderValue, ACCEPT},
    service::{service_fn, Service},
    Body, Error, Method, Request, Response, Server,
};
use interledger_btp::{connect_client, parse_btp_url};
use interledger_http::{HttpClientService, HttpServerService};
use interledger_ildcp::{get_ildcp_info, IldcpResponse, IldcpService};
use interledger_packet::{ErrorCode, RejectBuilder};
use interledger_router::Router;
use interledger_service::{incoming_service_fn, outgoing_service_fn};
use interledger_service_util::ValidatorService;
use interledger_spsp::{pay, spsp_responder};
use interledger_store_memory::{Account, AccountBuilder, InMemoryStore};
use interledger_stream::StreamReceiverService;
use ring::rand::{SecureRandom, SystemRandom};
use std::{net::SocketAddr, str, u64};
use tokio;
use url::Url;

const ACCOUNT_ID: u64 = 0;

#[doc(hidden)]
pub fn random_token() -> String {
    let mut bytes: [u8; 18] = [0; 18];
    SystemRandom::new().fill(&mut bytes).unwrap();
    base64::encode_config(&bytes, base64::URL_SAFE_NO_PAD)
}

#[doc(hidden)]
pub fn random_secret() -> [u8; 32] {
    let mut bytes: [u8; 32] = [0; 32];
    SystemRandom::new().fill(&mut bytes).unwrap();
    bytes
}

#[doc(hidden)]
pub fn send_spsp_payment_btp(btp_server: &str, receiver: &str, amount: u64, quiet: bool) {
    let receiver = receiver.to_string();
    let account = AccountBuilder::new()
        .additional_routes(&[&b""[..]])
        .btp_uri(Url::parse(btp_server).unwrap())
        .build();
    let store = InMemoryStore::from_accounts(vec![account.clone()]);
    let run = connect_client(
        incoming_service_fn(|_| Err(RejectBuilder::new(ErrorCode::F02_UNREACHABLE).build())),
        outgoing_service_fn(|_| Err(RejectBuilder::new(ErrorCode::F02_UNREACHABLE).build())),
        store.clone(),
        vec![ACCOUNT_ID],
    )
    .map_err(|err| {
        eprintln!("Error connecting to BTP server: {:?}", err);
        eprintln!("(Hint: is moneyd running?)");
    })
    .and_then(move |service| {
        // TODO seems kind of janky to clone the btp_service just to
        // close it later. Is there some better way of making sure it closes?
        let btp_service = service.clone();
        let service = ValidatorService::outgoing(service);
        let router = Router::new(service, store);
        pay(router, account, &receiver, amount)
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
                btp_service.close();
                Ok(())
            })
    });
    tokio::run(run);
}

#[doc(hidden)]
pub fn send_spsp_payment_http(http_server: &str, receiver: &str, amount: u64, quiet: bool) {
    let receiver = receiver.to_string();
    let url = Url::parse(http_server).expect("Cannot parse HTTP URL");
    let auth_header = if !url.username().is_empty() {
        Some(format!(
            "Basic {}",
            base64::encode(&format!(
                "{}:{}",
                url.username(),
                url.password().unwrap_or("")
            ))
        ))
    } else if let Some(password) = url.password() {
        Some(format!("Bearer {}", password))
    } else {
        None
    };
    let account = if let Some(auth_header) = auth_header {
        AccountBuilder::new()
            .additional_routes(&[&b""[..]])
            .http_endpoint(Url::parse(http_server).unwrap())
            .http_outgoing_authorization(auth_header)
            .build()
    } else {
        AccountBuilder::new()
            .additional_routes(&[&b""[..]])
            .http_endpoint(Url::parse(http_server).unwrap())
            .build()
    };
    let store = InMemoryStore::from_accounts(vec![account.clone()]);
    let service = HttpClientService::new(store.clone());
    let service = ValidatorService::outgoing(service);
    let service = Router::new(service, store);
    let run = pay(service, account, &receiver, amount)
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
        });
    tokio::run(run);
}

// TODO allow server secret to be specified
#[doc(hidden)]
pub fn run_spsp_server_btp(btp_server: &str, address: SocketAddr, quiet: bool) {
    let account: Account = AccountBuilder::new()
        .additional_routes(&[&b""[..]])
        .btp_uri(parse_btp_url(btp_server).unwrap())
        .build();
    let secret = random_secret();
    let store = InMemoryStore::from_accounts(vec![account.clone()]);
    let stream_server = StreamReceiverService::without_ildcp(
        &secret,
        incoming_service_fn(|_| Err(RejectBuilder::new(ErrorCode::F02_UNREACHABLE).build())),
    );

    let run = connect_client(
        ValidatorService::incoming(stream_server.clone()),
        outgoing_service_fn(|_| Err(RejectBuilder::new(ErrorCode::F02_UNREACHABLE).build())),
        store.clone(),
        vec![ACCOUNT_ID],
    )
    .map_err(|err| {
        eprintln!("Error connecting to BTP server: {:?}", err);
        eprintln!("(Hint: is moneyd running?)");
    })
    .and_then(move |btp_service| {
        let btp_service = ValidatorService::outgoing(btp_service);
        let mut router = Router::new(btp_service, store);
        get_ildcp_info(&mut router, account.clone()).and_then(move |info| {
            let client_address = Bytes::from(info.client_address());

            stream_server.set_ildcp(info);

            if !quiet {
                println!("Listening on: {}", address);
            }
            Server::bind(&address)
                .serve(move || spsp_responder(&client_address[..], &secret[..]))
                .map_err(|e| eprintln!("Server error: {:?}", e))
        })
    });
    tokio::run(run);
}

#[doc(hidden)]
pub fn run_spsp_server_http(
    ildcp_info: IldcpResponse,
    address: SocketAddr,
    auth_token: String,
    quiet: bool,
) {
    if !quiet {
        println!(
            "Creating SPSP server. ILP Address: {}",
            str::from_utf8(ildcp_info.client_address()).expect("ILP address is not valid UTF8")
        )
    }
    let account: Account = AccountBuilder::new()
        .http_incoming_authorization(format!("Bearer {}", auth_token))
        .build();
    let secret = random_secret();
    let store = InMemoryStore::from_accounts(vec![account.clone()]);
    let spsp_responder = spsp_responder(&ildcp_info.client_address(), &secret[..]);
    let incoming_handler = StreamReceiverService::new(
        &secret,
        ildcp_info,
        incoming_service_fn(|_| Err(RejectBuilder::new(ErrorCode::F02_UNREACHABLE).build())),
    );
    let incoming_handler = IldcpService::new(incoming_handler);
    let incoming_handler = ValidatorService::incoming(incoming_handler);
    let http_service = HttpServerService::new(incoming_handler, store);

    if !quiet {
        println!("Listening on: {}", address);
    }
    let server = Server::bind(&address)
        .serve(move || {
            let mut spsp_responder = spsp_responder.clone();
            let mut http_service = http_service.clone();
            service_fn(
                move |req: Request<Body>| -> Box<Future<Item = Response<Body>, Error = Error> + Send> {
                    match (req.method(), req.uri().path(), req.headers().get(ACCEPT)) {
                        (&Method::GET, "/spsp", _) => Box::new(spsp_responder.call(req)),
                        (&Method::GET, "/.well-known/pay", _) => Box::new(spsp_responder.call(req)),
                        (&Method::POST, "/ilp", _) => Box::new(http_service.call(req)),
                        (&Method::GET, _, Some(accept_header)) => {
                            if accept_header == HeaderValue::from_static("application/spsp4+json") {
                                Box::new(spsp_responder.call(req))
                            } else {
                        Box::new(ok(Response::builder()
                            .status(404)
                            .body(Body::empty())
                            .unwrap()))
                            }
                        },
                        _ => Box::new(ok(Response::builder()
                            .status(404)
                            .body(Body::empty())
                            .unwrap())),
                    }
                },
            )
        })
        .map_err(|err| eprintln!("Server error: {:?}", err));
    tokio::run(server);
}
