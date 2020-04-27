use super::{packet::*, BtpAccount, BtpStore};
use super::{service::BtpOutgoingService, wrapped_ws::WsWrap};
use futures::{FutureExt, Sink, Stream};
use futures::{SinkExt, StreamExt, TryFutureExt};
use interledger_service::*;
use secrecy::{ExposeSecret, SecretString};
use std::time::Duration;
use tracing::{debug, error, warn};
use warp::{
    self,
    ws::{Message, WebSocket, Ws},
    Filter,
};

// Close the incoming websocket connection if the auth details
// have not been received within this timeout
const WEBSOCKET_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_MESSAGE_SIZE: usize = 40000;

/// Returns a Warp Filter instantiated for the provided BtpOutgoingService service.
///
/// The warp filter handles the websocket upgrades and adds incoming connections
/// to the BTP service so that it will handle each of the messages.
pub fn btp_service_as_filter<O, S, A>(
    service: BtpOutgoingService<O, A>,
    store: S,
) -> warp::filters::BoxedFilter<(impl warp::Reply,)>
where
    O: OutgoingService<A> + Clone + Send + Sync + 'static,
    S: BtpStore<Account = A> + Clone + Send + Sync + 'static,
    A: BtpAccount + Send + Sync + 'static,
{
    warp::path("accounts")
        .and(warp::path::param::<Username>())
        .and(warp::path("ilp"))
        .and(warp::path("btp"))
        .and(warp::path::end())
        .and(warp::ws())
        .map(move |username: Username, ws: Ws| {
            // warp Websocket
            let service_clone = service.clone();
            let store_clone = store.clone();
            ws.max_message_size(MAX_MESSAGE_SIZE)
                .on_upgrade(|socket: WebSocket| {
                    // wrapper over tungstenite Websocket
                    add_connections(socket, username, service_clone, store_clone)
                        .map(|result| result.unwrap())
                })
        })
        .boxed()
}

/// This wraps a warp Websocket connection to make it act like a
/// tungstenite Websocket connection. It is needed for
/// compatibility with the BTP service that interacts with the
/// websocket implementation from warp and tokio-tungstenite
async fn add_connections<O, S, A>(
    socket: WebSocket,
    username: Username,
    service: BtpOutgoingService<O, A>,
    store: S,
) -> Result<(), ()>
where
    O: OutgoingService<A> + Clone + Send + Sync + 'static,
    S: BtpStore<Account = A> + Clone + Send + Sync + 'static,
    A: BtpAccount + Send + Sync + 'static,
{
    // We ignore all the errors
    let socket = socket.filter_map(|v| async move { v.ok() });
    let (account, connection) =
        match tokio::time::timeout(WEBSOCKET_TIMEOUT, validate_auth(store, username, socket)).await
        {
            Ok(res) => match res {
                Ok(res) => res,
                Err(_) => {
                    warn!("Closing Websocket connection because of invalid credentials");
                    return Ok(());
                }
            },
            Err(_) => {
                warn!("Closing Websocket connection because of an error");
                return Ok(());
            }
        };

    // We need to wrap our Warp connection in order to cast the Sink type
    // to tungstenite::Message. This probably can be implemented with SinkExt::with
    // but couldn't figure out how.
    service.add_connection(account.clone(), WsWrap { connection });
    debug!(
        "Added connection for account {}: (id: {})",
        account.username(),
        account.id()
    );

    Ok(())
}

struct Auth {
    request_id: u32,
    token: SecretString,
}

async fn validate_auth<S, A>(
    store: S,
    username: Username,
    connection: impl Stream<Item = Message> + Sink<Message>,
) -> Result<(A, impl Stream<Item = Message> + Sink<Message>), ()>
where
    S: BtpStore<Account = A> + 'static,
    A: BtpAccount + 'static,
{
    let (auth, mut connection) = get_auth(Box::pin(connection)).await?;
    debug!("Got BTP connection for username: {}", username);
    let account = store
        .get_account_from_btp_auth(&username, &auth.token.expose_secret())
        .map_err(move |_| warn!("BTP connection does not correspond to an account"))
        .await?;

    let auth_response = Message::binary(
        BtpResponse {
            request_id: auth.request_id,
            protocol_data: Vec::new(),
        }
        .to_bytes(),
    );

    connection
        .send(auth_response)
        .map_err(|_| error!("warp::Error sending auth response"))
        .await?;

    Ok((account, connection))
}

/// Reads the first non-empty non-error binary message from the WebSocket and attempts to parse it as an AuthToken
async fn get_auth(
    connection: impl Stream<Item = Message> + Sink<Message> + Unpin,
) -> Result<(Auth, impl Stream<Item = Message> + Sink<Message>), ()> {
    // Skip non-binary messages like Pings and Pongs
    // Note that the BTP protocol spec technically specifies that
    // the auth message MUST be the first packet sent over the
    // WebSocket connection. However, the JavaScript implementation
    // of BTP sends a Ping packet first, so we should ignore it.
    // (Be liberal in what you accept but strict in what you send)
    // TODO: should we error if the client sends something other than a binary or ping packet first?
    let mut connection =
        connection.skip_while(move |message| futures::future::ready(!message.is_binary()));

    // The first packet sent on the connection MUST be the auth packet
    let message = connection.next().await;
    match parse_auth(message) {
        Some(auth) => Ok((auth, Box::pin(connection))),
        None => {
            warn!("Got a BTP connection where the first packet sent was not a valid BTP Auth message. Closing the connection");
            Err(())
        }
    }
}

#[allow(clippy::cognitive_complexity)]
fn parse_auth(ws_packet: Option<Message>) -> Option<Auth> {
    if let Some(message) = ws_packet {
        if message.is_binary() {
            match BtpMessage::from_bytes(message.as_bytes()) {
                Ok(message) => {
                    let request_id = message.request_id;
                    let mut token: Option<String> = None;
                    // The primary data should be the "auth" with empty data
                    // The secondary data MUST have the "auth_token" with the authorization
                    // token set as the data field
                    for protocol_data in message.protocol_data.iter() {
                        let protocol_name: &str = protocol_data.protocol_name.as_ref();
                        if protocol_name == "auth_token" {
                            token = String::from_utf8(protocol_data.data.clone()).ok();
                        }
                    }

                    if let Some(token) = token {
                        return Some(Auth {
                            request_id,
                            token: SecretString::new(token),
                        });
                    } else {
                        warn!("BTP packet is missing auth token");
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
