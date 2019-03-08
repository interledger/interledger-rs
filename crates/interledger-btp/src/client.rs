use super::packet::*;
use super::service::BtpService;
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

// TODO does A need to be static?
pub fn connect_client<S, T, U, A: 'static>(
    incoming_handler: S,
    next_outgoing: T,
    store: U,
    accounts: Vec<<U::Account as Account>::AccountId>,
) -> impl Future<Item = BtpService<S, T, A>, Error = ()>
where
    S: IncomingService<U::Account> + Clone + Send + Sync + 'static,
    T: OutgoingService<A> + Clone + 'static,
    U: AccountStore<Account = A>,
    A: BtpAccount,
    // TODO do these need to be cloneable?
{
    store
        .get_accounts(accounts)
        .and_then(|accounts| {
            join_all(accounts.into_iter().map(move |account| {
                let mut url = account
                    .get_btp_uri()
                    .expect("Accounts must have BTP URLs")
                    .clone();
                if url.scheme().starts_with("btp+") {
                    url.set_scheme(&url.scheme().replace("btp+", "")).unwrap();
                }
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
                                        protocol_name: String::from("auth_username"),
                                        content_type: ContentType::TextPlainUtf8,
                                        data: String::from(url.username()).into_bytes(),
                                    },
                                    ProtocolData {
                                        protocol_name: String::from("auth_token"),
                                        content_type: ContentType::TextPlainUtf8,
                                        data: String::from(url.password().unwrap()).into_bytes(),
                                    },
                                ],
                            })
                            .to_bytes(),
                        );

                        connection.send(auth_packet).map_err(move |_| {
                            error!("Error sending auth packet on connection: {}", url)
                        })
                    })
                    .and_then(move |connection| Ok((account, connection)))
            }))
        })
        .map_err(|_err| ())
        .and_then(|connections| {
            let service = BtpService::new(incoming_handler, next_outgoing);
            for (account, connection) in connections.into_iter() {
                service.add_connection(account, connection);
            }
            Ok(service)
        })
}
