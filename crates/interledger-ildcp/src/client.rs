use super::packet::*;
use futures::Future;
use interledger_service::*;

pub fn get_ildcp_info<S, A>(
    service: &mut S,
    account: A,
) -> impl Future<Item = IldcpResponse, Error = ()>
where
    S: IncomingService<A>,
    A: Account,
{
    let prepare = IldcpRequest {}.to_prepare();
    service
        .handle_request(IncomingRequest {
            from: account,
            prepare,
        })
        .map_err(|err| error!("Error getting ILDCP info: {:?}", err))
        .and_then(|fulfill| {
            let response =
                IldcpResponse::try_from(fulfill.into_data().freeze()).map_err(|err| {
                    error!(
                        "Unable to parse ILDCP response from fulfill packet: {:?}",
                        err
                    );
                })?;
            trace!("Got ILDCP response: {:?}", response);
            Ok(response)
        })
}
