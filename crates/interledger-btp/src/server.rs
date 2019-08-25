use super::{
    packet::*, BtpAccount, BtpOpenSignupAccount, BtpOpenSignupStore, BtpOutgoingService, BtpStore,
};
use base64;
use futures::{future::result, Future, Sink, Stream};
use interledger_ildcp::IldcpResponse;
use interledger_service::*;
use log::{debug, error, warn};
use ring::digest::{digest, SHA256};
use std::{net::SocketAddr, str, str::FromStr};
use tokio_executor::spawn;
use tokio_tcp::{TcpListener, TcpStream};
use tokio_tungstenite::{
    accept_async_with_config, stream::Stream as WsStreamType, MaybeTlsStream, WebSocketStream,
};
use tungstenite::{
    protocol::{Message, WebSocketConfig},
    Error as WebSocketError,
};

type WsStream = WebSocketStream<MaybeTlsStream<TcpStream>>;

const MAX_MESSAGE_SIZE: usize = 40000;

/// Returns a BtpOutgoingService that wraps all BTP/WebSocket connections that come
/// in on the given address. Calling `handle_incoming` with an `IncomingService` will
/// turn the returned BtpOutgoingService into a bidirectional handler.
///
/// The separation is designed to enable the returned BtpOutgoingService to be passed
/// to another service like the Router, and _then_ for the Router to be passed as the
/// IncomingService to the BTP server.
pub fn create_server<T, U, A>(
    address: SocketAddr,
    store: U,
    next_outgoing: T,
) -> impl Future<Item = BtpOutgoingService<T, A>, Error = ()>
where
    T: OutgoingService<A> + Clone + Send + Sync + 'static,
    U: BtpStore<Account = A> + Clone + Send + Sync + 'static,
    A: BtpAccount + 'static,
{
    result(TcpListener::bind(&address).map_err(|err| {
        error!("Error binding to address {:?} {:?}", address, err);
    }))
    .and_then(move |socket| {
        debug!("Listening on {}", address);
        let service = BtpOutgoingService::new(next_outgoing);

        let service_clone = service.clone();
        let handle_incoming = socket
            .incoming()
            .map_err(|err| error!("Error handling incoming connection: {:?}", err))
            .for_each(move |stream| {
                let service_clone = service_clone.clone();
                let store = store.clone();
                accept_async_with_config(
                    WsStreamType::Plain(stream),
                    Some(WebSocketConfig {
                        max_send_queue: None,
                        max_message_size: Some(MAX_MESSAGE_SIZE),
                        max_frame_size: None,
                    }),
                )
                .map_err(|err| error!("Error accepting incoming WebSocket connection: {:?}", err))
                .and_then(|connection: WsStream| validate_auth(store, connection))
                .and_then(move |(account, connection)| {
                    debug!("Added connection for account {}", account.id());
                    service_clone.add_connection(account, connection);
                    Ok(())
                })
                .or_else(|_| {
                    warn!("Closing Websocket connection because it encountered an error");
                    Ok(())
                })
            })
            .then(move |result| {
                debug!("Finished listening for incoming BTP/Websocket connections");
                result
            });
        spawn(handle_incoming);

        Ok(service)
    })
}

/// Same as `create_server` but it returns a BTP server that will accept new connections
/// and create account records on the fly.
///
/// **WARNING:** Users of this should be very careful to prevent malicious users from creating huge numbers of accounts.
pub fn create_open_signup_server<T, U, A>(
    address: SocketAddr,
    ildcp_info: IldcpResponse,
    store: U,
    next_outgoing: T,
) -> impl Future<Item = BtpOutgoingService<T, A>, Error = ()>
where
    T: OutgoingService<A> + Clone + Send + Sync + 'static,
    U: BtpStore<Account = A> + BtpOpenSignupStore<Account = A> + Clone + Send + Sync + 'static,
    A: BtpAccount + 'static,
{
    result(TcpListener::bind(&address).map_err(|err| {
        error!("Error binding to address {:?} {:?}", address, err);
    }))
    .and_then(|socket| {
        let service = BtpOutgoingService::new(next_outgoing);

        let service_clone = service.clone();
        let handle_incoming = socket
            .incoming()
            .map_err(|err| error!("Error handling incoming connection: {:?}", err))
            .for_each(move |stream| {
                let service_clone = service_clone.clone();
                let store = store.clone();
                let ildcp_info = ildcp_info.clone();
                accept_async_with_config(
                    WsStreamType::Plain(stream),
                    Some(WebSocketConfig {
                        max_send_queue: None,
                        max_message_size: Some(MAX_MESSAGE_SIZE),
                        max_frame_size: None,
                    }),
                )
                .map_err(|err| error!("Error accepting incoming WebSocket connection: {:?}", err))
                .and_then(move |connection: WsStream| {
                    get_or_create_account(store, ildcp_info, connection)
                })
                .and_then(move |(account, connection)| {
                    debug!("Added connection for account: {:?}", account);
                    service_clone.add_connection(account, connection);
                    Ok(())
                })
                .or_else(|_| {
                    warn!("Closing Websocket connection because it encountered an error");
                    Ok(())
                })
            })
            .then(move |result| {
                debug!("Finished listening for incoming BTP/Websocket connections");
                result
            });
        spawn(handle_incoming);

        Ok(service)
    })
}

struct Auth {
    request_id: u32,
    username: Option<String>,
    token: String,
}

fn validate_auth<U, A>(
    store: U,
    connection: impl Stream<Item = Message, Error = WebSocketError>
        + Sink<SinkItem = Message, SinkError = WebSocketError>,
) -> impl Future<
    Item = (
        A,
        impl Stream<Item = Message, Error = WebSocketError>
            + Sink<SinkItem = Message, SinkError = WebSocketError>,
    ),
    Error = (),
>
where
    U: BtpStore<Account = A> + 'static,
    A: BtpAccount + 'static,
{
    get_auth(connection).and_then(move |(auth, connection)| {
        result(AuthToken::from_str(&auth.token).map_err(|_| ())).and_then(move |auth_token| {
            store
                .get_account_from_btp_token(auth_token.username().clone(), &auth_token.password())
                .map_err(move |_| {
                    warn!("Got BTP connection that does not correspond to an account")
                })
                .and_then(move |account| {
                    let auth_response = Message::Binary(
                        BtpResponse {
                            request_id: auth.request_id,
                            protocol_data: Vec::new(),
                        }
                        .to_bytes(),
                    );
                    connection
                        .send(auth_response)
                        .map_err(|_err| error!("Error sending auth response"))
                        .and_then(|connection| Ok((account, connection)))
                })
        })
    })
}

fn get_or_create_account<A, U>(
    store: U,
    ildcp_info: IldcpResponse,
    connection: impl Stream<Item = Message, Error = WebSocketError>
        + Sink<SinkItem = Message, SinkError = WebSocketError>,
) -> impl Future<
    Item = (
        A,
        impl Stream<Item = Message, Error = WebSocketError>
            + Sink<SinkItem = Message, SinkError = WebSocketError>,
    ),
    Error = (),
>
where
    U: BtpStore<Account = A> + BtpOpenSignupStore<Account = A> + 'static,
    A: BtpAccount + 'static,
{
    get_auth(connection).and_then(move |(auth, connection)| {
        let request_id = auth.request_id;
        result(AuthToken::from_str(&auth.token).map_err(|_| ())).and_then(move |auth_token| {
            store
                .get_account_from_btp_token(auth_token.username().clone(), &auth_token.password())
                .or_else(move |_| {
                    let local_part = if let Some(username) = auth.username {
                        username
                    } else {
                        base64::encode_config(
                            digest(&SHA256, auth.token.as_str().as_bytes()).as_ref(),
                            base64::URL_SAFE_NO_PAD,
                        )
                    };
                    let ilp_address = ildcp_info.client_address();
                    // in case local_part is set to be `auth.username`, will it always be a valid suffix?
                    // if we unwrap on the `with_suffix` call we implicitly allow auth.username to be an invalid suffix
                    let ilp_address = ilp_address.with_suffix(local_part.as_ref()).unwrap();

                    store
                        .create_btp_account(BtpOpenSignupAccount {
                            auth_token: &auth.token,
                            ilp_address: &ilp_address,
                            asset_code: str::from_utf8(ildcp_info.asset_code())
                                .expect("Asset code provided is not valid utf8"),
                            asset_scale: ildcp_info.asset_scale(),
                        })
                        .and_then(|account| {
                            debug!("Created new account: {:?}", account);
                            Ok(account)
                        })
                })
                .and_then(move |account| {
                    let auth_response = Message::Binary(
                        BtpResponse {
                            request_id,
                            protocol_data: Vec::new(),
                        }
                        .to_bytes(),
                    );
                    connection
                        .send(auth_response)
                        .map_err(|_err| error!("Error sending auth response"))
                        .and_then(|connection| Ok((account, connection)))
                })
        })
    })
}

fn get_auth(
    connection: impl Stream<Item = Message, Error = WebSocketError>
        + Sink<SinkItem = Message, SinkError = WebSocketError>,
) -> impl Future<
    Item = (
        Auth,
        impl Stream<Item = Message, Error = WebSocketError>
            + Sink<SinkItem = Message, SinkError = WebSocketError>,
    ),
    Error = (),
> {
    connection
        .skip_while(|message| {
            match message {
                // Skip non-binary messages like Pings and Pongs
                // Note that the BTP protocol spec technically specifies that
                // the auth message MUST be the first packet sent over the 
                // WebSocket connection. However, the JavaScript implementation
                // of BTP sends a Ping packet first, so we should ignore it.
                // (Be liberal in what you accept but strict in what you send)
                Message::Binary(_) => Ok(false),
                // TODO: should we error if the client sends something other than a binary or ping packet first?
                _ => Ok(true),
            }
        })
        .into_future()
        .map_err(|_err| ())
        .and_then(move |(message, connection)| {
            // The first packet sent on the connection MUST be the auth packet
            result(parse_auth(message).map(|auth| (auth, connection)).ok_or_else(|| {
                warn!("Got a BTP connection where the first packet sent was not a valid BTP Auth message. Closing the connection")
            }))
        })
}

fn parse_auth(ws_packet: Option<Message>) -> Option<Auth> {
    if let Some(Message::Binary(message)) = ws_packet {
        match BtpMessage::from_bytes(&message) {
            Ok(message) => {
                let request_id = message.request_id;
                let mut username: Option<String> = None;
                let mut token: Option<String> = None;
                for protocol_data in message.protocol_data.iter() {
                    match (
                        protocol_data.protocol_name.as_ref(),
                        !protocol_data.data.is_empty(),
                    ) {
                        ("auth_token", _) => {
                            token = String::from_utf8(protocol_data.data.clone()).ok()
                        }
                        ("auth_username", true) => {
                            username = String::from_utf8(protocol_data.data.clone()).ok()
                        }
                        _ => {}
                    }
                }

                if let Some(token) = token {
                    return Some(Auth {
                        request_id,
                        token,
                        username,
                    });
                } else {
                    warn!("BTP packet is missing auth token");
                }
            }
            Err(err) => {
                warn!("Error parsing BTP packet from Websocket message: {:?}", err);
            }
        }
    } else {
        warn!("Websocket packet is not binary");
    }
    None
}
