use super::service::{BtpOutgoingService, WsError};
use super::{packet::*, BtpAccount, BtpStore};
use futures::{future::result, Async, AsyncSink, Future, Poll, Sink, Stream};
use interledger_packet::Address;
use interledger_service::*;
use log::{debug, error, warn};
use std::{str::FromStr, time::Duration};
use tokio_timer::Timeout;
use tungstenite;
use warp::{
    self,
    ws::{Message, WebSocket, Ws2},
    Filter,
};

// Close the incoming websocket connection if the auth details
// have not been received within this timeout
const WEBSOCKET_TIMEOUT: Duration = Duration::from_secs(10);

// const MAX_MESSAGE_SIZE: usize = 40000;

/// Returns a BtpOutgoingService and a warp Filter.
///
/// The BtpOutgoingService wraps all BTP/WebSocket connections that come
/// in on the given address. Calling `handle_incoming` with an `IncomingService` will
/// turn the returned BtpOutgoingService into a bidirectional handler.
/// The separation is designed to enable the returned BtpOutgoingService to be passed
/// to another service like the Router, and _then_ for the Router to be passed as the
/// IncomingService to the BTP server.
///
/// The warp filter handles the websocket upgrades and adds incoming connections
/// to the BTP service so that it will handle each of the messages.
pub fn create_btp_service_and_filter<O, S, A>(
    ilp_address: Address,
    store: S,
    next_outgoing: O,
) -> (
    BtpOutgoingService<O, A>,
    warp::filters::BoxedFilter<(impl warp::Reply,)>,
)
where
    O: OutgoingService<A> + Clone + Send + Sync + 'static,
    S: BtpStore<Account = A> + Clone + Send + Sync + 'static,
    A: BtpAccount + 'static,
{
    let service = BtpOutgoingService::new(ilp_address, next_outgoing);
    let service_clone = service.clone();
    let filter = warp::ws2()
        .map(move |ws: Ws2| {
            let store = store.clone();
            let service_clone = service_clone.clone();
            ws.on_upgrade(move |ws: WebSocket| {
                // TODO set max_message_size once https://github.com/seanmonstar/warp/pull/272 is merged
                let service_clone = service_clone.clone();
                Timeout::new(validate_auth(store, ws), WEBSOCKET_TIMEOUT)
                    .and_then(move |(account, connection)| {
                        debug!(
                            "Added connection for account {}: (id: {})",
                            account.username(),
                            account.id()
                        );
                        service_clone.add_connection(account, WsWrap { connection });
                        Ok(())
                    })
                    .or_else(|_| {
                        warn!("Closing Websocket connection because of an error");
                        Ok(())
                    })
            })
        })
        .boxed();
    (service, filter)
}

/// This wraps a warp Websocket connection to make it act like a
/// tungstenite Websocket connection. It is needed for
/// compatibility with the BTP service that interacts with the
/// websocket implementation from warp and tokio-tungstenite
struct WsWrap<W> {
    connection: W,
}

impl<W> Stream for WsWrap<W>
where
    W: Stream<Item = Message, Error = warp::Error>
        + Sink<SinkItem = Message, SinkError = warp::Error>,
{
    type Item = tungstenite::Message;
    type Error = WsError;

    fn poll(&mut self) -> Poll<Option<Self::Item>, Self::Error> {
        match self.connection.poll() {
            Ok(Async::NotReady) => Ok(Async::NotReady),
            Ok(Async::Ready(None)) => Ok(Async::Ready(None)),
            Ok(Async::Ready(Some(message))) => {
                let message = if message.is_ping() {
                    tungstenite::Message::Ping(message.into_bytes())
                } else if message.is_binary() {
                    tungstenite::Message::Binary(message.into_bytes())
                } else if message.is_text() {
                    tungstenite::Message::Text(message.to_str().unwrap_or_default().to_string())
                } else if message.is_close() {
                    tungstenite::Message::Close(None)
                } else {
                    warn!(
                        "Got unexpected websocket message, closing connection: {:?}",
                        message
                    );
                    tungstenite::Message::Close(None)
                };
                Ok(Async::Ready(Some(message)))
            }
            Err(err) => Err(WsError::from(err)),
        }
    }
}

impl<W> Sink for WsWrap<W>
where
    W: Stream<Item = Message, Error = warp::Error>
        + Sink<SinkItem = Message, SinkError = warp::Error>,
{
    type SinkItem = tungstenite::Message;
    type SinkError = WsError;

    fn start_send(
        &mut self,
        item: Self::SinkItem,
    ) -> Result<AsyncSink<Self::SinkItem>, Self::SinkError> {
        match item {
            tungstenite::Message::Binary(data) => self
                .connection
                .start_send(Message::binary(data))
                .map(|result| {
                    if let AsyncSink::NotReady(message) = result {
                        AsyncSink::NotReady(tungstenite::Message::Binary(message.into_bytes()))
                    } else {
                        AsyncSink::Ready
                    }
                })
                .map_err(WsError::from),
            tungstenite::Message::Text(data) => {
                match self.connection.start_send(Message::text(data)) {
                    Ok(AsyncSink::NotReady(message)) => {
                        if let Ok(string) = String::from_utf8(message.into_bytes()) {
                            Ok(AsyncSink::NotReady(tungstenite::Message::text(string)))
                        } else {
                            Err(WsError::Tungstenite(tungstenite::Error::Utf8))
                        }
                    }
                    Ok(AsyncSink::Ready) => Ok(AsyncSink::Ready),
                    Err(err) => Err(WsError::from(err)),
                }
            }
            // Ignore other message types because warp's WebSocket type doesn't
            // allow us to send any other types of messages
            // TODO make sure warp's websocket responds to pings and/or sends them to keep the
            // connection alive
            _ => Ok(AsyncSink::Ready),
        }
    }

    fn poll_complete(&mut self) -> Poll<(), Self::SinkError> {
        self.connection.poll_complete().map_err(WsError::from)
    }
}

struct Auth {
    request_id: u32,
    token: AuthToken,
}

fn validate_auth<S, A>(
    store: S,
    connection: impl Stream<Item = Message, Error = warp::Error>
        + Sink<SinkItem = Message, SinkError = warp::Error>,
) -> impl Future<
    Item = (
        A,
        impl Stream<Item = Message, Error = warp::Error>
            + Sink<SinkItem = Message, SinkError = warp::Error>,
    ),
    Error = (),
>
where
    S: BtpStore<Account = A> + 'static,
    A: BtpAccount + 'static,
{
    get_auth(connection).and_then(move |(auth, connection)| {
        debug!("Got BTP connection for username: {}", auth.token.username());
        store
            .get_account_from_btp_auth(&auth.token.username(), &auth.token.password())
            .map_err(move |_| warn!("BTP connection does not correspond to an account"))
            .and_then(move |account| {
                let auth_response = Message::binary(
                    BtpResponse {
                        request_id: auth.request_id,
                        protocol_data: Vec::new(),
                    }
                    .to_bytes(),
                );
                connection
                    .send(auth_response)
                    .map_err(|_err| error!("warp::Error sending auth response"))
                    .and_then(|connection| Ok((account, connection)))
            })
    })
}

fn get_auth(
    connection: impl Stream<Item = Message, Error = warp::Error>
        + Sink<SinkItem = Message, SinkError = warp::Error>,
) -> impl Future<
    Item = (
        Auth,
        impl Stream<Item = Message, Error = warp::Error>
            + Sink<SinkItem = Message, SinkError = warp::Error>,
    ),
    Error = (),
> {
    connection
        .skip_while(|message| {
            // Skip non-binary messages like Pings and Pongs
            // Note that the BTP protocol spec technically specifies that
            // the auth message MUST be the first packet sent over the 
            // WebSocket connection. However, the JavaScript implementation
            // of BTP sends a Ping packet first, so we should ignore it.
            // (Be liberal in what you accept but strict in what you send)
            Ok(!message.is_binary())
            // TODO: should we error if the client sends something other than a binary or ping packet first?
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
    if let Some(message) = ws_packet {
        if message.is_binary() {
            match BtpMessage::from_bytes(message.as_bytes()) {
                Ok(message) => {
                    let request_id = message.request_id;
                    let mut username: Option<String> = None;
                    let mut token: Option<String> = None;
                    for protocol_data in message.protocol_data.iter() {
                        let protocol_name: &str = protocol_data.protocol_name.as_ref();
                        if protocol_name == "auth_token" {
                            token = String::from_utf8(protocol_data.data.clone()).ok();
                        } else if protocol_name == "auth_username" {
                            username = String::from_utf8(protocol_data.data.clone()).ok();
                        }
                    }

                    match (username, token) {
                        (Some(ref username), Some(ref token)) => {
                            return AuthToken::new(username, token)
                                .ok()
                                .map(|token| Auth { request_id, token });
                        }
                        (None, Some(ref token)) => {
                            return AuthToken::from_str(token)
                                .ok()
                                .map(|token| Auth { request_id, token });
                        }
                        _ => warn!("BTP packet is missing auth token"),
                    }
                }
                Err(err) => {
                    warn!(
                        "warp::Error parsing BTP packet from Websocket message: {:?}",
                        err
                    );
                }
            }
        } else {
            warn!("Websocket packet is not binary");
        }
    }
    None
}
