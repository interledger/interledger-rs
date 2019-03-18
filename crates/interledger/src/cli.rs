use base64;
use bytes::Bytes;
use futures::{future::ok, Future};
use hyper::{
    header::{HeaderValue, ACCEPT},
    service::{service_fn, Service},
    Body, Error, Method, Request, Response, Server,
};
use interledger_api::{NodeApi, NodeStore};
use interledger_btp::{connect_client, create_open_signup_server, create_server, parse_btp_url};
use interledger_http::{HttpClientService, HttpServerService};
use interledger_ildcp::{get_ildcp_info, IldcpAccount, IldcpResponse, IldcpService};
use interledger_packet::{ErrorCode, RejectBuilder};
use interledger_router::Router;
use interledger_service::{
    incoming_service_fn, outgoing_service_fn, IncomingRequest, OutgoingRequest,
};
use interledger_service_util::{
    ExchangeRateAndBalanceService, MaxPacketAmountService, ValidatorService,
};
use interledger_spsp::{pay, spsp_responder};
use interledger_store_memory::{Account, AccountBuilder, InMemoryStore};
use interledger_store_redis::connect as connect_redis_store;
use interledger_stream::StreamReceiverService;
use parking_lot::RwLock;
use ring::rand::{SecureRandom, SystemRandom};
use std::{net::SocketAddr, str, sync::Arc, u64};
use tokio::{self, net::TcpListener};
use tower_web::ServiceBuilder;
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
pub fn send_spsp_payment_btp(
    btp_server: &str,
    receiver: &str,
    amount: u64,
    quiet: bool,
) -> impl Future<Item = (), Error = ()> {
    let receiver = receiver.to_string();
    let account = AccountBuilder::new()
        .additional_routes(&[&b""[..]])
        .btp_uri(Url::parse(btp_server).unwrap())
        .build();
    let store = InMemoryStore::from_accounts(vec![account.clone()]);
    connect_client(
        incoming_service_fn(|_| {
            Err(RejectBuilder {
                code: ErrorCode::F02_UNREACHABLE,
                message: b"Not expecting incoming prepare packets",
                triggered_by: &[],
                data: &[],
            }
            .build())
        }),
        outgoing_service_fn(|request: OutgoingRequest<Account>| {
            Err(RejectBuilder {
                code: ErrorCode::F02_UNREACHABLE,
                message: &format!(
                    "No route found for address: {}",
                    str::from_utf8(&request.from.client_address()[..]).unwrap_or("<not utf8>")
                )
                .as_bytes(),
                triggered_by: &[],
                data: &[],
            }
            .build())
        }),
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
    })
}

#[doc(hidden)]
pub fn send_spsp_payment_http(
    http_server: &str,
    receiver: &str,
    amount: u64,
    quiet: bool,
) -> impl Future<Item = (), Error = ()> {
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
    pay(service, account, &receiver, amount)
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
}

// TODO allow server secret to be specified
#[doc(hidden)]
pub fn run_spsp_server_btp(
    btp_server: &str,
    address: SocketAddr,
    quiet: bool,
) -> impl Future<Item = (), Error = ()> {
    debug!("Starting SPSP server");
    let ilp_address = Arc::new(RwLock::new(Bytes::new()));
    let account: Account = AccountBuilder::new()
        .additional_routes(&[&b""[..]])
        .btp_uri(parse_btp_url(btp_server).unwrap())
        .build();
    let secret = random_secret();
    let store = InMemoryStore::from_accounts(vec![account.clone()]);
    let ilp_address_clone = ilp_address.clone();
    let stream_server = StreamReceiverService::without_ildcp(
        &secret,
        incoming_service_fn(move |request: IncomingRequest<Account>| {
            Err(RejectBuilder {
                code: ErrorCode::F02_UNREACHABLE,
                message: &format!(
                    "No handler configured for destination: {}",
                    str::from_utf8(&request.from.client_address()[..]).unwrap_or("<not utf8>")
                )
                .as_bytes(),
                triggered_by: &ilp_address_clone.read()[..],
                data: &[],
            }
            .build())
        }),
    );

    let ilp_address_read_clone = ilp_address.clone();
    let ilp_address_write_clone = ilp_address.clone();
    connect_client(
        ValidatorService::incoming(stream_server.clone()),
        outgoing_service_fn(move |request: OutgoingRequest<Account>| {
            Err(RejectBuilder {
                code: ErrorCode::F02_UNREACHABLE,
                message: &format!(
                    "No outgoing route for: {}",
                    str::from_utf8(&request.from.client_address()[..]).unwrap_or("<not utf8>")
                )
                .as_bytes(),
                triggered_by: &ilp_address_read_clone.read()[..],
                data: &[],
            }
            .build())
        }),
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
            *ilp_address_write_clone.write() = client_address.clone();

            stream_server.set_ildcp(info);

            if !quiet {
                println!("Listening on: {}", address);
            }
            Server::bind(&address)
                .serve(move || spsp_responder(&client_address[..], &secret[..]))
                .map_err(|e| eprintln!("Server error: {:?}", e))
        })
    })
}

#[doc(hidden)]
pub fn run_spsp_server_http(
    ildcp_info: IldcpResponse,
    address: SocketAddr,
    auth_token: String,
    quiet: bool,
) -> impl Future<Item = (), Error = ()> {
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
    let ilp_address = Bytes::from(ildcp_info.client_address());
    let incoming_handler = StreamReceiverService::new(
        &secret,
        ildcp_info,
        incoming_service_fn(move |request: IncomingRequest<Account>| {
            Err(RejectBuilder {
                code: ErrorCode::F02_UNREACHABLE,
                message: &format!(
                    "No handler configured for destination: {}",
                    str::from_utf8(&request.from.client_address()[..]).unwrap_or("<not utf8>")
                )
                .as_bytes(),
                triggered_by: &ilp_address[..],
                data: &[],
            }
            .build())
        }),
    );
    let incoming_handler = IldcpService::new(incoming_handler);
    let incoming_handler = ValidatorService::incoming(incoming_handler);
    let http_service = HttpServerService::new(incoming_handler, store);

    if !quiet {
        println!("Listening on: {}", address);
    }
    Server::bind(&address)
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
        .map_err(|err| eprintln!("Server error: {:?}", err))
}

#[doc(hidden)]
pub fn run_moneyd_local(
    address: SocketAddr,
    ildcp_info: IldcpResponse,
) -> impl Future<Item = (), Error = ()> {
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
    create_open_signup_server(address, ildcp_info, store.clone(), rejecter).and_then(
        move |btp_service| {
            let service = Router::new(btp_service.clone(), store);
            let service = IldcpService::new(service);
            let service = ValidatorService::incoming(service);
            btp_service.handle_incoming(service);
            Ok(())
        },
    )
}

#[doc(hidden)]
// TODO when a BTP connection is made, insert a outgoing HTTP entry into the Store to tell other
// connector instances to forward packets for that account to us
pub fn run_node_redis(
    redis_uri: &str,
    btp_address: SocketAddr,
    http_address: SocketAddr,
    server_secret: &[u8; 32],
) -> impl Future<Item = (), Error = ()> {
    debug!("Starting Interledger node with Redis store");
    let server_secret = Bytes::from(&server_secret[..]);
    connect_redis_store(redis_uri)
        .map_err(|err| eprintln!("Error connecting to Redis: {:?}", err))
        .and_then(move |store| {
            let http_outgoing = HttpClientService::new(store.clone());
            create_server(btp_address, store.clone(), http_outgoing).and_then(move |btp_service| {
                let outgoing_service = btp_service.clone();
                let outgoing_service =
                    ExchangeRateAndBalanceService::new(store.clone(), outgoing_service);

                let incoming_service = Router::new(outgoing_service, store.clone());
                let incoming_service = IldcpService::new(incoming_service);
                let incoming_service = MaxPacketAmountService::new(incoming_service);
                let incoming_service = ValidatorService::incoming(incoming_service);

                // Handle packets sent via BTP
                btp_service.handle_incoming(incoming_service.clone());

                // TODO should this run the node api on a different port so it's easier to separate public/private?
                // Note the API also includes receiving ILP packets sent via HTTP
                let api = NodeApi::new(server_secret, store.clone(), incoming_service.clone());
                let listener =
                    TcpListener::bind(&http_address).expect("Unable to bind to HTTP address");
                println!("Interledger node listening on: {}", http_address);
                let server = ServiceBuilder::new()
                    .resource(api)
                    .serve(listener.incoming());
                tokio::spawn(server);
                Ok(())
            })
        })
}

#[doc(hidden)]
pub use interledger_api::AccountDetails;
#[doc(hidden)]
pub fn insert_account_redis(
    redis_uri: &str,
    account: AccountDetails,
) -> impl Future<Item = (), Error = ()> {
    connect_redis_store(redis_uri)
        .map_err(|err| eprintln!("Error connecting to Redis: {:?}", err))
        .and_then(move |store| {
            store
                .insert_account(account)
                .map_err(|_| eprintln!("Unable to create account"))
                .and_then(|account| {
                    // TODO add quiet option
                    println!("Created account: {:?}", account);
                    Ok(())
                })
        })
}
