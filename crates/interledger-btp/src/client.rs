use super::packet::*;
use super::service::BtpOutgoingService;
use super::BtpAccount;
use futures::{future::join_all, SinkExt, StreamExt, TryFutureExt};
use interledger_errors::ApiError;
use interledger_packet::Address;
use interledger_service::*;
use rand::random;
use thiserror::Error;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;
use tracing::{debug, error, trace};
use url::Url;

/// Create a BtpOutgoingService wrapping BTP connections to the accounts specified.
/// Calling `handle_incoming` with an `IncomingService` will turn the returned
/// BtpOutgoingService into a bidirectional handler.
pub async fn connect_client<A, S>(
    ilp_address: Address,
    accounts: Vec<A>,
    error_on_unavailable: bool,
    next_outgoing: S,
) -> Result<BtpOutgoingService<S, A>, BtpClientError>
where
    S: OutgoingService<A> + Clone + 'static,
    A: BtpAccount + Send + Sync + 'static,
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
    let res = join_all(connect_btp).await;
    if res.into_iter().any(|r| r.is_err()) {
        return Err(BtpClientError::CannotConnectMultiple);
    }
    Ok(service)
}

#[derive(Error, Debug)]
pub enum BtpClientError {
    #[error("Cannot connect to BTP url: {0}. Got error {1}")]
    CannotConnect(String, Url, String),
    #[error("Account is unavailable. Error: {0}")]
    Unavailable(String),
    #[error("Could not connect to at least one BTP account ")]
    CannotConnectMultiple,
}

impl From<BtpClientError> for warp::Rejection {
    fn from(src: BtpClientError) -> Self {
        let err = ApiError::internal_server_error().detail(src.to_string());
        warp::reject::custom(err)
    }
}

/// Initiates a BTP connection with the specified account and saves it to the list of connections
/// maintained by the provided service. This is done in the following steps:
/// 1. Initialize a WebSocket connection at the BTP account's URL
/// 2. Send a BTP authorization packet to the peer
/// 3. If successful, consider the BTP connection established and add it to the service
pub async fn connect_to_service_account<O, A>(
    account: A,
    error_on_unavailable: bool,
    service: BtpOutgoingService<O, A>,
) -> Result<(), BtpClientError>
where
    O: OutgoingService<A> + Clone + 'static,
    A: BtpAccount + Send + Sync + 'static,
{
    let account_id = account.id();
    let mut url = account
        .get_ilp_over_btp_url()
        .expect("Accounts must have BTP URLs")
        .clone();
    if url.scheme().starts_with("btp+") {
        // Re-parse the URL after stripping off the leading "btp+" prefix.
        // We cannot use set_scheme here because the URL specification
        // does not allow converting between "special" and "non-special"
        // schemes, and "ws" is considered special.
        // The unwrap cannot fail since we've already been given a valid
        // URL, and in this branch we know it begins with "btp+".
        url = Url::parse(&url.into_string()[4..]).unwrap();
    }
    let token = account
        .get_ilp_over_btp_outgoing_token()
        .map(|s| s.to_vec())
        .unwrap_or_default();
    debug!("Connecting to {}", url);

    let (mut connection, _) = connect_async(url.clone())
        .map_err(|err| {
            BtpClientError::CannotConnect(
                account.username().to_string(),
                url.clone(),
                err.to_string(),
            )
        })
        .await?;

    trace!(
        "Connected to account {} (UID: {}) (URI: {}), sending auth packet",
        account.username(),
        account_id,
        url
    );

    // Send BTP authentication
    let auth_packet = Message::binary(
        BtpPacket::Message(BtpMessage {
            request_id: random(),
            protocol_data: vec![
                ProtocolData {
                    protocol_name: "auth".into(),
                    content_type: ContentType::ApplicationOctetStream,
                    data: vec![],
                },
                ProtocolData {
                    protocol_name: "auth_token".into(),
                    content_type: ContentType::TextPlainUtf8,
                    data: token,
                },
            ],
        })
        .to_bytes(),
    );

    // (right now we just assume they'll close the connection if the auth didn't work)
    let result = connection // this just a stream
        .send(auth_packet)
        .await;

    match result {
        Ok(_) => {
            debug!("Connected to account {}'s server", account.id());
            let connection = connection.filter_map(|v| async move { v.ok() });
            service.add_connection(account, connection);
            Ok(())
        }
        Err(err) => {
            let msg = format!("Error sending auth packet on connection {}: {}", url, err);
            error!("{}", msg);
            if error_on_unavailable {
                Err(BtpClientError::Unavailable(msg))
            } else {
                Ok(())
            }
        }
    }
}
