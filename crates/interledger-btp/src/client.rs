use super::packet::*;
use super::service::BtpOutgoingService;
use super::BtpAccount;
use futures::{future::join_all, Future, Sink, Stream};
use interledger_packet::Address;
use interledger_service::*;
use log::{debug, error, trace};
use rand::random;
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
    ilp_address: Address,
    accounts: Vec<A>,
    error_on_unavailable: bool,
    next_outgoing: S,
) -> impl Future<Item = BtpOutgoingService<S, A>, Error = ()>
where
    S: OutgoingService<A> + Clone + 'static,
    A: BtpAccount + 'static,
{
    let service = BtpOutgoingService::new(ilp_address, next_outgoing);
    let mut connect_btp = Vec::new();
    for account in accounts {
        // Can we make this take a reference to a service?
        connect_btp.push(connect_to_service_account(
            account,
            error_on_unavailable,
            service.clone(),
        ));
    }
    join_all(connect_btp).and_then(move |_| Ok(service))
}

pub fn connect_to_service_account<O, A>(
    account: A,
    error_on_unavailable: bool,
    service: BtpOutgoingService<O, A>,
) -> impl Future<Item = (), Error = ()>
where
    O: OutgoingService<A> + Clone + 'static,
    A: BtpAccount + 'static,
{
    let account_id = account.id();
    let mut url = account
        .get_ilp_over_btp_url()
        .expect("Accounts must have BTP URLs")
        .clone();
    if url.scheme().starts_with("btp+") {
        url.set_scheme(&url.scheme().replace("btp+", "")).unwrap();
    }
    let token = account
        .get_ilp_over_btp_outgoing_token()
        .map(|s| s.to_vec())
        .unwrap_or_default();
    debug!("Connecting to {}", url);
    connect_async(url.clone())
        .map_err(move |err| {
            error!(
                "Error connecting to WebSocket server for account: {} {:?}",
                account_id, err
            )
        })
        .and_then(move |(connection, _)| {
            trace!(
                "Connected to account {} (URI: {}), sending auth packet",
                account_id,
                url
            );
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

            // TODO check that the response is a success before proceeding
            // (right now we just assume they'll close the connection if the auth didn't work)
            connection
                .send(auth_packet)
                .map_err(move |_| error!("Error sending auth packet on connection: {}", url))
                .then(move |result| match result {
                    Ok(connection) => {
                        debug!("Connected to account {}'s server", account.id());
                        let connection = connection.from_err().sink_from_err();
                        service.add_connection(account, connection);
                        Ok(())
                    }
                    Err(_) => {
                        if error_on_unavailable {
                            Err(())
                        } else {
                            Ok(())
                        }
                    }
                })
        })
}
