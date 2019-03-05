use super::{packet::*, BtpAccount, BtpService, BtpStore};
use futures::{future::result, Future, Sink, Stream};
use interledger_service::*;
use std::net::SocketAddr;
use tokio_executor::spawn;
use tokio_tcp::TcpListener;
use tokio_tungstenite::{accept_async_with_config, stream::Stream as MaybeTlsStream};
use tungstenite::protocol::{Message, WebSocketConfig};

const MAX_MESSAGE_SIZE: usize = 40000;

pub fn create_server<S, T, U, A>(
    address: SocketAddr,
    store: U,
    incoming_handler: S,
    next_outgoing: T,
) -> impl Future<Item = BtpService<S, T, A>, Error = ()>
where
    S: IncomingService<A> + Clone + Send + Sync + 'static,
    T: OutgoingService<A> + Clone + Send + Sync + 'static,
    U: BtpStore<Account = A> + Clone + Send + Sync + 'static,
    A: BtpAccount + 'static,
{
    result(TcpListener::bind(&address).map_err(|err| {
        error!("Error binding to address {:?} {:?}", address, err);
    }))
    .and_then(|socket| {
        let service = BtpService::new(incoming_handler, next_outgoing);

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
                    debug!("Added connection for account: {:?}", account);
                    service_clone.add_connection(account, connection);
                    Ok(())
                })
            });
        spawn(handle_incoming);

        Ok(service)
    })
}

fn validate_auth<U, C, A>(store: U, connection: C) -> impl Future<Item = (A, C), Error = ()>
where
    U: BtpStore<Account = A> + 'static,
    C: Stream<Item = Message> + Sink<SinkItem = Message>,
    A: BtpAccount + 'static,
{
    connection
        .into_future()
        .map_err(|_err| ())
        .and_then(move |(message, connection)| {
            // The first packet sent on the connection MUST be the auth packet
            result(parse_auth(message).ok_or(())).and_then(move |auth| {
                store
                    .get_account_from_auth(&auth.token, auth.username.as_ref().map(|s| &**s))
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

struct Auth {
    request_id: u32,
    username: Option<String>,
    token: String,
}

fn parse_auth(ws_packet: Option<Message>) -> Option<Auth> {
    if let Some(Message::Binary(message)) = ws_packet {
        if let Ok(message) = BtpMessage::from_bytes(&message) {
            let request_id = message.request_id;
            let mut username: Option<String> = None;
            let mut token: Option<String> = None;
            for protocol_data in message.protocol_data.iter() {
                match protocol_data.protocol_name.as_ref() {
                    "auth_token" => token = String::from_utf8(protocol_data.data.clone()).ok(),
                    "auth_username" => {
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
