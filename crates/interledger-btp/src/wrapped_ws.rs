use futures::stream::Stream;
use futures::Sink;
use pin_project::pin_project;
use std::pin::Pin;
use std::task::{Context, Poll};
use tokio_tungstenite::tungstenite;
use tracing::warn;
use warp::ws::Message;

/// Wrapper struct to unify the Tungstenite WebSocket connection from connect_async
/// with the Warp websocket connection from ws.upgrade. Stream and Sink are re-implemented
/// for this struct, normalizing it to use Tungstenite's messages and a wrapped error type
#[pin_project]
#[derive(Clone)]
pub(crate) struct WsWrap<W> {
    #[pin]
    pub(crate) connection: W,
}

impl<W> Stream for WsWrap<W>
where
    W: Stream<Item = Message>,
{
    type Item = tokio_tungstenite::tungstenite::Message;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context) -> Poll<Option<Self::Item>> {
        let this = self.project();
        match this.connection.poll_next(cx) {
            Poll::Pending => Poll::Pending,
            Poll::Ready(val) => match val {
                Some(v) => {
                    let v = convert_msg(v);
                    Poll::Ready(Some(v))
                }
                None => Poll::Ready(None),
            },
        }
    }
}

impl<W> Sink<tokio_tungstenite::tungstenite::Message> for WsWrap<W>
where
    W: Sink<Message>,
{
    type Error = W::Error;

    fn poll_ready(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        let this = self.project();
        this.connection.poll_ready(cx)
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
        this.connection.start_send(item)
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        let this = self.project();
        this.connection.poll_flush(cx)
    }

    fn poll_close(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        let this = self.project();
        this.connection.poll_close(cx)
    }
}

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
