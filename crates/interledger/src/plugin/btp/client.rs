use super::super::Plugin;
use super::{
    BtpMessage, BtpPacket, BtpPacketStream, BtpRequestIdCheckerStream, ContentType,
    IlpPacketStream, ProtocolData,
};
use futures::{Future, Poll, Sink, StartSend, Stream};
use ilp::{IlpFulfillmentChecker, IlpPacket};
use tokio_tcp::TcpStream;
use tokio_tungstenite::{connect_async as connect_websocket, MaybeTlsStream, WebSocketStream};
use url::Url;

pub type BtpStream = BtpPacketStream<WebSocketStream<MaybeTlsStream<TcpStream>>>;
pub type IlpRequest = (u32, IlpPacket);

pub struct ClientPlugin {
    inner: IlpFulfillmentChecker<IlpPacketStream<BtpRequestIdCheckerStream<BtpStream>>>,
}

impl Plugin for ClientPlugin {}

impl Stream for ClientPlugin {
    type Item = IlpRequest;
    type Error = ();

    fn poll(&mut self) -> Poll<Option<Self::Item>, ()> {
        self.inner.poll()
    }
}

impl Sink for ClientPlugin {
    type SinkItem = IlpRequest;
    type SinkError = ();

    fn start_send(&mut self, item: Self::SinkItem) -> StartSend<Self::SinkItem, Self::SinkError> {
        self.inner.start_send(item)
    }

    fn poll_complete(&mut self) -> Poll<(), Self::SinkError> {
        self.inner.poll_complete()
    }
}

pub fn connect_async(
    server: &str,
) -> impl Future<Item = ClientPlugin, Error = PluginBtpError> + 'static + Send {
    connect_btp_stream(server).and_then(|stream| {
        let with_id_checker = BtpRequestIdCheckerStream::new(stream);
        let with_ilp_parsing = IlpPacketStream::new(with_id_checker);
        let with_fulfillment_checker = IlpFulfillmentChecker::new(with_ilp_parsing);
        Ok(ClientPlugin {
            inner: with_fulfillment_checker,
        })
    })
}

pub fn connect_btp_stream(
    server: &str,
) -> impl Future<Item = BtpStream, Error = PluginBtpError> + 'static + Send {
    let server = server.replace("btp+", "");
    let server = Url::parse(&server).unwrap();
    let mut server_without_auth = server.clone();
    server_without_auth.set_username("").unwrap();
    server_without_auth.set_password(None).unwrap();

    let auth_packet = BtpPacket::Message(BtpMessage {
        request_id: 0,
        protocol_data: vec![
            ProtocolData {
                protocol_name: String::from("auth"),
                content_type: ContentType::ApplicationOctetStream,
                data: vec![],
            },
            ProtocolData {
                protocol_name: String::from("auth_username"),
                content_type: ContentType::TextPlainUtf8,
                data: String::from(server.username()).into_bytes(),
            },
            ProtocolData {
                protocol_name: String::from("auth_token"),
                content_type: ContentType::TextPlainUtf8,
                data: String::from(server.password().unwrap()).into_bytes(),
            },
        ],
    });

    debug!("Connecting WebSocket: {}", &server_without_auth);
    connect_websocket(server)
        .map_err(|err| PluginBtpError(format!("Error connecting to websocket: {:?}", err)))
        .and_then(|(ws, _handshake)| Ok(BtpPacketStream::new(ws)))
        .and_then(move |plugin| {
            plugin
                .send(auth_packet)
                .map_err(|err| PluginBtpError(format!("Error sending auth packet: {:?}", err)))
                .and_then(move |plugin| {
                    plugin
                        .into_future()
                        .and_then(move |(_auth_response, plugin)| {
                            info!("Connected to server: {}", server_without_auth);
                            Ok(plugin)
                        })
                        .map_err(|(err, _plugin)| {
                            PluginBtpError(format!("Error getting auth response: {:?}", err))
                        })
                })
        })
}

#[derive(Fail, Debug)]
#[fail(display = "Error connecting to BTP server: {}", _0)]
pub struct PluginBtpError(String);
