use super::packet::*;
use futures::future::TryFutureExt;
use interledger_service::*;
use std::convert::TryFrom;
use tracing::{debug, error};

/// Sends an ILDCP Request to the provided service from the provided account
/// and receives the account's ILP address and asset details
pub async fn get_ildcp_info<S, A>(service: &mut S, account: A) -> Result<IldcpResponse, ()>
where
    S: IncomingService<A>,
    A: Account,
{
    let prepare = IldcpRequest {}.to_prepare();
    let fulfill = service
        .handle_request(IncomingRequest {
            from: account,
            prepare,
        })
        .map_err(|err| error!("Error getting ILDCP info: {:?}", err))
        .await?;

    let response = IldcpResponse::try_from(fulfill.into_data().freeze()).map_err(|err| {
        error!(
            "Unable to parse ILDCP response from fulfill packet: {:?}",
            err
        );
    })?;
    debug!("Got ILDCP response: {:?}", response);
    Ok(response)
}
