use super::{IlpPacket, IlpRequest, Plugin};
use crate::ildcp::IldcpResponse;
use futures::sync::mpsc::{unbounded, UnboundedReceiver, UnboundedSender};
use futures::{Poll, Sink, StartSend, Stream};

pub fn create_mock_plugins() -> (MockPlugin, MockPlugin) {
    let (a_tx, a_rx) = unbounded();
    let (b_tx, b_rx) = unbounded();

    let a = MockPlugin {
        address: String::from("example.pluginA"),
        outgoing: a_tx.clone(),
        incoming: b_rx,
        incoming_sender: b_tx.clone(),
    };
    let b = MockPlugin {
        address: String::from("example.pluginB"),
        outgoing: b_tx,
        incoming: a_rx,
        incoming_sender: a_tx,
    };
    (a, b)
}

pub struct MockPlugin {
    address: String,
    outgoing: UnboundedSender<IlpRequest>,
    incoming: UnboundedReceiver<IlpRequest>,
    incoming_sender: UnboundedSender<IlpRequest>,
}

impl Stream for MockPlugin {
    type Item = IlpRequest;
    type Error = ();

    fn poll(&mut self) -> Poll<Option<Self::Item>, Self::Error> {
        self.incoming.poll()
    }
}

impl Sink for MockPlugin {
    type SinkItem = IlpRequest;
    type SinkError = ();

    fn start_send(&mut self, item: Self::SinkItem) -> StartSend<Self::SinkItem, Self::SinkError> {
        // Handle ILDCP requests
        if let IlpPacket::Prepare(ref prepare) = item.1 {
            if prepare.destination == "peer.config" {
                let ildcp_response = IldcpResponse::new(&self.address, 9, "XYZ")
                    .to_fulfill()
                    .unwrap();
                return self
                    .incoming_sender
                    .start_send((item.0, IlpPacket::Fulfill(ildcp_response)))
                    .map_err(|_err| ());
            }
        }

        self.outgoing.start_send(item).map_err(|_err| ())
    }

    fn poll_complete(&mut self) -> Poll<(), Self::SinkError> {
        self.outgoing.poll_complete().map_err(|_err| ())
    }
}

impl Plugin for MockPlugin {}
