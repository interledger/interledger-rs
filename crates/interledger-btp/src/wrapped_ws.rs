use futures::stream::Stream;
use futures::Sink;
use log::warn;
use pin_project::pin_project;
use std::error::Error;
use std::fmt;
use std::pin::Pin;
use std::task::{Context, Poll};
use warp::ws::Message;

/// Wrapper struct to unify the Tungstenite WebSocket connection from connect_async
/// with the Warp websocket connection from ws.upgrade. Stream and Sink are re-implemented
/// for this struct, normalizing it to use Tungstenite's messages and a wrapped error type
#[pin_project]
#[derive(Clone)]
pub struct WsWrap<W> {
    #[pin]
    connection: W,
}

impl<W> Stream for WsWrap<W>
where
    W: Stream<Item = Result<Message, warp::Error>>,
{
    type Item = Result<tungstenite::Message, WsError>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context) -> Poll<Option<Self::Item>> {
        let this = self.project();
        match this.connection.poll_next(cx) {
            Poll::Pending => Poll::Pending,
            Poll::Ready(val) => {
                match val {
                    Some(v) => {
                        let v = match v {
                            // We could do a map_ok / map_err here but we wouldn't
                            // be able to convert the Sink to the desired data type
                            Ok(msg) => Ok(convert_msg(msg)),
                            Err(err) => Err(WsError::from(err)),
                        };
                        Poll::Ready(Some(v))
                    }
                    None => Poll::Ready(None),
                }
            }
        }
    }
}

impl<W, E> Sink<tungstenite::Message> for WsWrap<W>
where
    W: Sink<Message, Error = E>,
    E: Into<WsError>,
{
    type Error = WsError;

    fn poll_ready(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        let this = self.project();
        this.connection.poll_ready(cx).map_err(|e| e.into())
    }

    fn start_send(self: Pin<&mut Self>, item: tungstenite::Message) -> Result<(), Self::Error> {
        let this = self.project();
        let item = match item {
            tungstenite::Message::Binary(data) => Message::binary(data),
            tungstenite::Message::Text(data) => Message::text(data),
            // Ignore other message types because warp's WebSocket type doesn't
            // allow us to send any other types of messages
            // TODO make sure warp's websocket responds to pings and/or sends them to keep the
            // connection alive
            _ => return Ok(()),
        };
        this.connection.start_send(item).map_err(|e| e.into())
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        let this = self.project();
        this.connection.poll_flush(cx).map_err(|e| e.into())
    }

    fn poll_close(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        let this = self.project();
        this.connection.poll_close(cx).map_err(|e| e.into())
    }
}

// Implement From<tungstenite::WebSocket> and From<warp::ws::WebSocket> ?

fn convert_msg(message: Message) -> tungstenite::Message {
    if message.is_ping() {
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
    }
}

#[derive(Debug)]
pub enum WsError {
    Tungstenite(tungstenite::Error),
    Warp(warp::Error),
}

impl fmt::Display for WsError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            WsError::Tungstenite(err) => err.fmt(f),
            WsError::Warp(err) => err.fmt(f),
        }
    }
}

impl Error for WsError {}

impl From<tungstenite::Error> for WsError {
    fn from(err: tungstenite::Error) -> Self {
        WsError::Tungstenite(err)
    }
}

impl From<warp::Error> for WsError {
    fn from(err: warp::Error) -> Self {
        WsError::Warp(err)
    }
}
