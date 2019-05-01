use super::{
    packet::*, BtpAccount, BtpOpenSignupAccount, BtpOpenSignupStore, BtpOutgoingService, BtpStore,
};
use base64;
use bytes::{BufMut, Bytes, BytesMut};
use futures::{future::result, Future, Sink, Stream};
use interledger_ildcp::IldcpResponse;
use interledger_service::*;
use ring::digest::{digest, SHA256};
use std::{net::SocketAddr, str};
use tokio_executor::spawn;
use tokio_tcp::TcpListener;
use tokio_tungstenite::{accept_async_with_config, stream::Stream as MaybeTlsStream};
use tungstenite::protocol::{Message, WebSocketConfig};

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
                    MaybeTlsStream::Plain(stream),
                    Some(WebSocketConfig {
                        max_send_queue: None,
                        max_message_size: Some(MAX_MESSAGE_SIZE),
                        max_frame_size: None,
                    }),
                )
                .map_err(|err| error!("Error accepting incoming WebSocket connection: {:?}", err))
                .and_then(|connection| validate_auth(store, connection))
                .and_then(move |(account, connection)| {
                    debug!("Added connection for account {}", account.id());
                    service_clone.add_connection(account, connection);
                    Ok(())
                })
            })
            .then(move |result| {
                debug!("Finished reading connections from TcpListener");
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
                    MaybeTlsStream::Plain(stream),
                    Some(WebSocketConfig {
                        max_send_queue: None,
                        max_message_size: Some(MAX_MESSAGE_SIZE),
                        max_frame_size: None,
                    }),
                )
                .map_err(|err| error!("Error accepting incoming WebSocket connection: {:?}", err))
                .and_then(move |connection| get_or_create_account(store, ildcp_info, connection))
                .and_then(move |(account, connection)| {
                    debug!("Added connection for account: {:?}", account);
                    service_clone.add_connection(account, connection);
                    Ok(())
                })
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

fn validate_auth<U, C, A>(store: U, connection: C) -> impl Future<Item = (A, C), Error = ()>
where
    U: BtpStore<Account = A> + 'static,
    C: Stream<Item = Message> + Sink<SinkItem = Message>,
    A: BtpAccount + 'static,
{
    get_auth(connection).and_then(move |(auth, connection)| {
        let token = auth.token.clone();
        store
            .get_account_from_btp_token(&auth.token)
            .map_err(move |_| warn!("Got unauthorized connection with token: {}", token))
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
}

fn get_or_create_account<A, C, U>(
    store: U,
    ildcp_info: IldcpResponse,
    connection: C,
) -> impl Future<Item = (A, C), Error = ()>
where
    U: BtpStore<Account = A> + BtpOpenSignupStore<Account = A> + 'static,
    C: Stream<Item = Message> + Sink<SinkItem = Message>,
    A: BtpAccount + 'static,
{
    get_auth(connection).and_then(move |(auth, connection)| {
        let request_id = auth.request_id;
        store
            .get_account_from_btp_token(&auth.token)
            .or_else(move |_| {
                let local_part: Bytes = if let Some(username) = auth.username {
                    Bytes::from(username)
                } else {
                    Bytes::from(base64::encode_config(
                        digest(&SHA256, auth.token.as_str().as_bytes()).as_ref(),
                        base64::URL_SAFE_NO_PAD,
                    ))
                };
                let mut ilp_address = BytesMut::with_capacity(
                    ildcp_info.client_address().len() + 1 + local_part.len(),
                );
                ilp_address.put(ildcp_info.client_address());
                ilp_address.put(&b"."[..]);
                ilp_address.put(local_part);
                store
                    .create_btp_account(BtpOpenSignupAccount {
                        auth_token: &auth.token,
                        ilp_address: &ilp_address[..],
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
}

fn get_auth<C>(connection: C) -> impl Future<Item = (Auth, C), Error = ()>
where
    C: Stream<Item = Message> + Sink<SinkItem = Message>,
{
    connection
        .into_future()
        .map_err(|_err| ())
        .and_then(move |(message, connection)| {
            // The first packet sent on the connection MUST be the auth packet
            result(parse_auth(message).map(|auth| (auth, connection)).ok_or(()))
        })
}

fn parse_auth(ws_packet: Option<Message>) -> Option<Auth> {
    if let Some(Message::Binary(message)) = ws_packet {
        if let Ok(message) = BtpMessage::from_bytes(&message) {
            let request_id = message.request_id;
            let mut username: Option<String> = None;
            let mut token: Option<String> = None;
            for protocol_data in message.protocol_data.iter() {
                match (
                    protocol_data.protocol_name.as_ref(),
                    !protocol_data.data.is_empty(),
                ) {
                    ("auth_token", _) => token = String::from_utf8(protocol_data.data.clone()).ok(),
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
            }
        }
    }
    None
}
