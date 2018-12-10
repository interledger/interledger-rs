use super::{deserialize_packet, BtpPacket, Serializable};
use futures::{Async, AsyncSink, Poll, StartSend};
use futures::{Sink, Stream};
use std::error::Error as StdError;

pub struct BtpPacketStream<S> {
    inner: S,
}

impl<S> BtpPacketStream<S> {
    pub fn new<I, E>(stream: S) -> Self
    where
        S: Stream<Item = I, Error = E> + Sink<SinkItem = I, SinkError = E>,
        I: Into<Vec<u8>> + Sized,
        E: StdError,
    {
        BtpPacketStream { inner: stream }
    }
}

impl<S> Stream for BtpPacketStream<S>
where
    S: Stream,
    S::Item: Into<Vec<u8>>,
    S::Error: StdError,
{
    type Item = BtpPacket;
    type Error = ();

    fn poll(&mut self) -> Poll<Option<BtpPacket>, Self::Error> {
        let poll_result = self
            .inner
            .poll()
            .map_err(|err| error!("Error polling: {:?}", err));
        if let Some(serialized) = try_ready!(poll_result) {
            let serialized_vec: Vec<u8> = serialized.into();
            if let Ok(packet) = deserialize_packet(&serialized_vec) {
                Ok(Async::Ready(Some(packet)))
            } else {
                warn!("Ignoring unknown BTP packet {:x?}", &serialized_vec);
                Ok(Async::NotReady)
            }
        } else {
            trace!("Stream ended");
            Ok(Async::Ready(None))
        }
    }
}

impl<S> Sink for BtpPacketStream<S>
where
    S: Sink,
    S::SinkItem: From<Vec<u8>>,
    S::SinkError: StdError,
{
    type SinkItem = BtpPacket;
    type SinkError = ();

    fn start_send(&mut self, item: BtpPacket) -> StartSend<Self::SinkItem, Self::SinkError> {
        let serialized = item.to_bytes();
        self.inner
            .start_send(serialized.into())
            .map(|result| match result {
                AsyncSink::Ready => AsyncSink::Ready,
                AsyncSink::NotReady(_) => {
                    debug!("BTP packet sink was not ready to send {:?}", item);
                    AsyncSink::NotReady(item)
                }
            })
            .map_err(|err| {
                error!("Error sending BTP packet: {}", err);
            })
    }

    fn poll_complete(&mut self) -> Result<Async<()>, Self::SinkError> {
        self.inner.poll_complete().map_err(|e| {
            error!("Polling error {}", e);
        })
    }
}
