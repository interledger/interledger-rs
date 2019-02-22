use super::packet::*;
use futures::Future;
use interledger_service::*;

pub fn get_ildcp_info<S>(
  service: &mut S,
  account_id: AccountId,
) -> impl Future<Item = IldcpResponse, Error = ()>
where
  S: IncomingService,
{
  let prepare = IldcpRequest {}.to_prepare();
  service
    .handle_request(IncomingRequest {
      from: account_id,
      prepare,
    })
    .map_err(|_| ())
    .and_then(|fulfill| IldcpResponse::try_from(fulfill.into_data().freeze()).map_err(|_| ()))
}
