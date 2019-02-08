use byteorder::{ReadBytesExt, WriteBytesExt};
use bytes::Bytes;
use chrono::{Duration, Utc};
use errors::ParseError;
use futures::{Async, AsyncSink, Future, Poll};
use ilp::{IlpFulfill, IlpPacket, IlpPrepare, IlpReject};
use oer::{ReadOerExt, WriteOerExt};
use plugin::{IlpRequest, Plugin};
use rand::random;
use std::io::Cursor;

static ILDCP_DESTINATION: &'static str = "peer.config";
lazy_static! {
    static ref PEER_PROTOCOL_EXPIRY_DURATION: Duration = Duration::minutes(1);
    static ref PEER_PROTOCOL_FULFILLMENT: Bytes = Bytes::from(vec![0; 32]);
    static ref PEER_PROTOCOL_CONDITION: Bytes = Bytes::from(vec![
        102, 104, 122, 173, 248, 98, 189, 119, 108, 143, 193, 139, 142, 159, 142, 32, 8, 151, 20,
        133, 110, 226, 51, 179, 144, 42, 89, 29, 13, 95, 41, 37
    ]);
}

#[derive(Debug, Default)]
pub struct IldcpRequest {}

impl IldcpRequest {
    pub fn new() -> Self {
        IldcpRequest {}
    }

    pub fn to_prepare(&self) -> IlpPrepare {
        IlpPrepare::new(
            ILDCP_DESTINATION,
            0,
            &PEER_PROTOCOL_CONDITION[..],
            Utc::now() + *PEER_PROTOCOL_EXPIRY_DURATION,
            Bytes::new(),
        )
    }
}

#[derive(Debug, PartialEq)]
pub struct IldcpResponse {
    pub client_address: String,
    pub asset_scale: u8,
    pub asset_code: String,
}

impl IldcpResponse {
    pub fn new(client_address: &str, asset_scale: u8, asset_code: &str) -> Self {
        IldcpResponse {
            client_address: client_address.to_string(),
            asset_scale,
            asset_code: asset_code.to_string(),
        }
    }

    pub fn from_fulfill(fulfill: &IlpFulfill) -> Result<Self, ParseError> {
        let mut reader = Cursor::new(&fulfill.data[..]);
        let client_address = String::from_utf8(reader.read_var_octet_string()?)?;
        let asset_scale = reader.read_u8()?;
        let asset_code = String::from_utf8(reader.read_var_octet_string()?)?;
        Ok(IldcpResponse {
            client_address,
            asset_scale,
            asset_code,
        })
    }

    pub fn to_fulfill(&self) -> Result<IlpFulfill, ParseError> {
        let mut writer = Vec::new();
        writer.write_var_octet_string(self.client_address.as_bytes())?;
        writer.write_u8(self.asset_scale)?;
        writer.write_var_octet_string(self.asset_code.as_bytes())?;
        Ok(IlpFulfill::new(&[0u8; 32][..], writer))
    }
}

// On error only returns the plugin if it can continue to be used
pub fn get_config(
    plugin: impl Plugin,
) -> impl Future<Item = (IldcpResponse, impl Plugin), Error = Error> {
    GetConfigFuture {
        plugin: Some(plugin),
        outgoing_request: None,
        sent_config_request: false,
        config_request_id: random(),
    }
}

// TODO timeout request in case we don't get a response back (we should because the packet will expire)
struct GetConfigFuture<P> {
    plugin: Option<P>,
    outgoing_request: Option<IlpRequest>,
    sent_config_request: bool,
    config_request_id: u32,
}

impl<P> GetConfigFuture<P>
where
    P: Plugin,
{
    fn try_send_outgoing(&mut self, request: IlpRequest) -> Poll<(), Error> {
        if let Some(ref mut plugin) = self.plugin {
            match plugin.start_send(request) {
                Ok(AsyncSink::NotReady(request)) => {
                    self.outgoing_request = Some(request);
                    Ok(Async::NotReady)
                }
                Ok(AsyncSink::Ready) => Ok(Async::Ready(())),
                Err(_) => Err(Error("Error sending request to plugin".to_string())),
            }
        } else {
            panic!("Polled after finish");
        }
    }
}

impl<P> Future for GetConfigFuture<P>
where
    P: Plugin,
{
    type Item = (IldcpResponse, P);
    type Error = Error;

    fn poll(&mut self) -> Poll<Self::Item, Self::Error> {
        loop {
            // Try sending outgoing packet
            if let Some(request) = self.outgoing_request.take() {
                try_ready!(self.try_send_outgoing(request));
            }

            // Send the config request
            if !self.sent_config_request {
                self.sent_config_request = true;
                let request_id = self.config_request_id;
                try_ready!(self.try_send_outgoing((
                    request_id,
                    IlpPacket::Prepare(IldcpRequest::new().to_prepare())
                )));
            }

            // Poll for the response, rejecting other packets with a T00 error
            let next = if let Some(ref mut plugin) = self.plugin {
                plugin.poll()
            } else {
                return Err(Error("Future polled after finish".to_string()));
            };
            match next {
                Ok(Async::Ready(Some((request_id, packet)))) => {
                    if request_id == self.config_request_id {
                        if let IlpPacket::Fulfill(ref fulfill) = packet {
                            match IldcpResponse::from_fulfill(&fulfill) {
                                Ok(response) => {
                                    return Ok(Async::Ready((
                                        response,
                                        self.plugin.take().unwrap(),
                                    )));
                                }
                                Err(err) => {
                                    return Err(Error(format!(
                                        "Unable to parse ILDCP response: {:?}",
                                        err
                                    )));
                                }
                            }
                        } else {
                            return Err(Error("Config request was rejected".to_string()));
                        }
                    } else {
                        if let IlpPacket::Prepare(_) = packet {
                            debug!("Rejecting incoming prepare packet while waiting for ILDCP response");
                            try_ready!(self.try_send_outgoing((
                                request_id,
                                IlpPacket::Reject(IlpReject::new("T00", "", "", Bytes::new()))
                            )));
                        } else {
                            warn!(
                                "Ignoring response packet while waiting for ILDCP response: {:?}",
                                packet
                            );
                        }
                        continue;
                    }
                }
                Ok(Async::Ready(None)) => {
                    return Err(Error(
                        "Plugin closed before ILDCP response was received".to_string(),
                    ));
                }
                Ok(Async::NotReady) => {
                    return Ok(Async::NotReady);
                }
                Err(_err) => {
                    return Err(Error(
                        "Error polling plugin for incoming requests".to_string(),
                    ));
                }
            }
        }
    }
}

#[derive(Fail, Debug)]
#[fail(display = "Error getting ILDCP info: {}", _0)]
pub struct Error(String);

#[cfg(test)]
mod tests {
    use super::*;

    mod get_config {
        use super::*;
        use crate::plugin::mock::{create_mock_plugins, SENDER_ADDRESS};
        use futures::{task, task::Task};
        use futures::{Async, AsyncSink, Poll, Sink, StartSend, Stream};

        #[test]
        fn gets_ildcp_details() {
            let (sender, _receiver) = create_mock_plugins();
            let (response, _plugin) = get_config(sender).wait().unwrap();
            assert_eq!(
                response,
                IldcpResponse {
                    client_address: SENDER_ADDRESS.to_string(),
                    asset_code: "XYZ".to_string(),
                    asset_scale: 9
                }
            )
        }

        #[test]
        fn rejects_other_packets() {
            let (sender, receiver) = create_mock_plugins();
            sender
                .incoming_sender
                .unbounded_send((
                    0,
                    IlpPacket::Prepare(IlpPrepare::new(
                        SENDER_ADDRESS,
                        100,
                        &[0; 32][..],
                        Utc::now() + Duration::seconds(30),
                        Bytes::new(),
                    )),
                ))
                .unwrap();
            let (ildcp_response, _plugin) = get_config(sender).wait().unwrap();
            assert_eq!(
                ildcp_response,
                IldcpResponse {
                    client_address: SENDER_ADDRESS.to_string(),
                    asset_code: "XYZ".to_string(),
                    asset_scale: 9
                }
            );
            let (response, _plugin) = receiver.into_future().wait().expect("Receiver got error");
            assert_eq!(
                response.unwrap(),
                (
                    0,
                    IlpPacket::Reject(IlpReject::new("T00", "", "", Bytes::new(),))
                )
            );
        }

        #[test]
        fn plugin_closed() {
            struct FakePlugin {};
            impl Sink for FakePlugin {
                type SinkItem = IlpRequest;
                type SinkError = ();

                fn start_send(
                    &mut self,
                    _item: Self::SinkItem,
                ) -> StartSend<Self::SinkItem, Self::SinkError> {
                    Ok(AsyncSink::Ready)
                }

                fn poll_complete(&mut self) -> Poll<(), Self::SinkError> {
                    Ok(Async::Ready(()))
                }
            }
            impl Stream for FakePlugin {
                type Item = IlpRequest;
                type Error = ();

                fn poll(&mut self) -> Poll<Option<Self::Item>, Self::Error> {
                    // Ends right away
                    Ok(Async::Ready(None))
                }
            }
            impl Plugin for FakePlugin {};

            let plugin = FakePlugin {};
            let result = get_config(plugin).wait();
            assert!(result.is_err());
        }

        #[test]
        fn plugin_poll_error() {
            struct FakePlugin {};
            impl Sink for FakePlugin {
                type SinkItem = IlpRequest;
                type SinkError = ();

                fn start_send(
                    &mut self,
                    _item: Self::SinkItem,
                ) -> StartSend<Self::SinkItem, Self::SinkError> {
                    Ok(AsyncSink::Ready)
                }

                fn poll_complete(&mut self) -> Poll<(), Self::SinkError> {
                    Ok(Async::Ready(()))
                }
            }
            impl Stream for FakePlugin {
                type Item = IlpRequest;
                type Error = ();

                fn poll(&mut self) -> Poll<Option<Self::Item>, Self::Error> {
                    Err(())
                }
            }
            impl Plugin for FakePlugin {};

            let plugin = FakePlugin {};
            let result = get_config(plugin).wait();
            assert!(result.is_err());
        }

        #[test]
        fn config_request_rejected() {
            struct FakePlugin {
                request_id: Option<u32>,
                poll_task: Option<Task>,
            };
            impl Sink for FakePlugin {
                type SinkItem = IlpRequest;
                type SinkError = ();

                fn start_send(
                    &mut self,
                    item: Self::SinkItem,
                ) -> StartSend<Self::SinkItem, Self::SinkError> {
                    self.request_id = Some(item.0);
                    if let Some(task) = self.poll_task.take() {
                        task.notify();
                    }
                    Ok(AsyncSink::Ready)
                }

                fn poll_complete(&mut self) -> Poll<(), Self::SinkError> {
                    Ok(Async::Ready(()))
                }
            }
            impl Stream for FakePlugin {
                type Item = IlpRequest;
                type Error = ();

                fn poll(&mut self) -> Poll<Option<Self::Item>, Self::Error> {
                    if let Some(request_id) = self.request_id.take() {
                        Ok(Async::Ready(Some((
                            request_id,
                            IlpPacket::Reject(IlpReject::new("F00", "", "", Bytes::new())),
                        ))))
                    } else {
                        self.poll_task = Some(task::current());
                        Ok(Async::NotReady)
                    }
                }
            }
            impl Plugin for FakePlugin {};

            let plugin = FakePlugin {
                request_id: None,
                poll_task: None,
            };
            let result = get_config(plugin).wait();
            assert!(result.is_err());
        }
    }

}
