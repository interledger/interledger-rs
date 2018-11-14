use super::IlpPacket;
use chrono::{DateTime, Utc};
use futures::{Async, Poll, Sink, StartSend, Stream};
use hex;
use ring::digest::{digest, SHA256};
use std::collections::HashMap;

pub struct IlpFulfillmentChecker<S> {
    inner: S,
    packets: HashMap<u32, (Vec<u8>, DateTime<Utc>)>,
}

impl<S> IlpFulfillmentChecker<S>
where
    S: Stream<Item = (u32, IlpPacket), Error = ()>
        + Sink<SinkItem = (u32, IlpPacket), SinkError = ()>,
{
    pub fn new(stream: S) -> Self {
        IlpFulfillmentChecker {
            inner: stream,
            packets: HashMap::new(),
        }
    }
}

impl<S> Stream for IlpFulfillmentChecker<S>
where
    S: Stream<Item = (u32, IlpPacket), Error = ()>
        + Sink<SinkItem = (u32, IlpPacket), SinkError = ()>,
{
    type Item = (u32, IlpPacket);
    type Error = ();

    fn poll(&mut self) -> Poll<Option<Self::Item>, Self::Error> {
        let item = try_ready!(self.inner.poll());
        match item {
            Some((request_id, IlpPacket::Fulfill(fulfill))) => {
                if let Some((condition, expires_at)) = self.packets.remove(&request_id) {
                    if !fulfillment_matches_condition(&fulfill.fulfillment, condition.as_ref()) {
                        warn!("Got invalid fulfillment with request id {}: {} (invalid fulfillment. original condition: {:x?})", request_id, hex::encode(&fulfill.fulfillment[..]), hex::encode(&condition[..]));
                        // TODO do this without removing / reinserting each time
                        self.packets.insert(request_id, (condition, expires_at));
                        Ok(Async::NotReady)
                    } else if expires_at < Utc::now() {
                        warn!(
                            "Got invalid Fulfill with request id {} (expired at: {})",
                            request_id,
                            expires_at.to_rfc3339()
                        );
                        // TODO do this without removing / reinserting each time
                        self.packets.insert(request_id, (condition, expires_at));
                        Ok(Async::NotReady)
                    } else {
                        trace!(
                            "Got valid Fulfill matching prepare with request id: {}: {}",
                            request_id,
                            hex::encode(&fulfill.fulfillment[..])
                        );
                        Ok(Async::Ready(Some((
                            request_id,
                            IlpPacket::Fulfill(fulfill),
                        ))))
                    }
                } else {
                    // We never saw the Prepare that corresponds to this
                    warn!(
                        "Got Fulfill for unknown request id {}: {}",
                        request_id,
                        hex::encode(&fulfill.fulfillment[..])
                    );
                    Ok(Async::NotReady)
                }
            }
            Some((request_id, IlpPacket::Reject(reject))) => {
                self.packets.remove(&request_id);
                Ok(Async::Ready(Some((request_id, IlpPacket::Reject(reject)))))
            }
            Some(item) => Ok(Async::Ready(Some(item))),
            None => {
                trace!("Stream ended");
                Ok(Async::Ready(None))
            }
        }
    }
}

impl<S> Sink for IlpFulfillmentChecker<S>
where
    S: Sink<SinkItem = (u32, IlpPacket), SinkError = ()>,
{
    type SinkItem = (u32, IlpPacket);
    type SinkError = ();

    fn start_send(&mut self, item: Self::SinkItem) -> StartSend<Self::SinkItem, Self::SinkError> {
        if let (request_id, IlpPacket::Prepare(prepare)) = &item {
            self.packets.insert(
                *request_id,
                (prepare.execution_condition.to_vec(), prepare.expires_at),
            );
        }

        self.inner.start_send(item)
    }

    fn poll_complete(&mut self) -> Poll<(), Self::SinkError> {
        self.inner.poll_complete()
    }
}

fn fulfillment_matches_condition(fulfillment: &[u8], condition: &[u8]) -> bool {
    digest(&SHA256, fulfillment).as_ref() == condition
}
