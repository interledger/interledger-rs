use super::packet::*;
use super::service::BtpOutgoingService;
use super::BtpAccount;
use futures::{future::join_all, Future, Sink};
use interledger_service::*;
use rand::random;
use std::iter::IntoIterator;
use tokio_tungstenite::connect_async;
use tungstenite::Message;
use url::{ParseError, Url};

pub fn parse_btp_url(uri: &str) -> Result<Url, ParseError> {
    let uri = if uri.starts_with("btp+") {
        uri.split_at(4).1
    } else {
        uri
    };
    Url::parse(uri)
}

/// Create a BtpOutgoingService wrapping BTP connections to the accounts specified.
/// Calling `handle_incoming` with an `IncomingService` will turn the returned
/// BtpOutgoingService into a bidirectional handler.
pub fn connect_client<A, S>(
    accounts: Vec<A>,
    next_outgoing: S,
) -> impl Future<Item = BtpOutgoingService<S, A>, Error = ()>
where
    S: OutgoingService<A> + Clone + 'static,
    A: BtpAccount + 'static,
{
    join_all(accounts.into_iter().map(move |account| {
        let mut url = account
            .get_btp_uri()
            .expect("Accounts must have BTP URLs")
            .clone();
        if url.scheme().starts_with("btp+") {
            url.set_scheme(&url.scheme().replace("btp+", "")).unwrap();
        }
        let token = account
            .get_btp_token()
            .map(|s| s.to_vec())
            .unwrap_or_default();
        debug!("Connecting to {}", url);
        connect_async(url.clone())
            .map_err(|err| error!("Error connecting to WebSocket server: {:?}", err))
            .and_then(move |(connection, _)| {
                debug!("Connected to {}, sending auth packet", url);
                // Send BTP authentication
                let auth_packet = Message::Binary(
                    BtpPacket::Message(BtpMessage {
                        request_id: random(),
                        protocol_data: vec![
                            ProtocolData {
                                protocol_name: String::from("auth"),
                                content_type: ContentType::ApplicationOctetStream,
                                data: vec![],
                            },
                            ProtocolData {
                                protocol_name: String::from("auth_token"),
                                content_type: ContentType::TextPlainUtf8,
                                data: token,
                            },
                        ],
                    })
                    .to_bytes(),
                );

                connection
                    .send(auth_packet)
                    .map_err(move |_| error!("Error sending auth packet on connection: {}", url))
            })
            .and_then(move |connection| Ok((account, connection)))
    }))
    .and_then(|connections| {
        let service = BtpOutgoingService::new(next_outgoing);
        for (account, connection) in connections.into_iter() {
            service.add_connection(account, connection);
        }
        Ok(service)
    })
}
