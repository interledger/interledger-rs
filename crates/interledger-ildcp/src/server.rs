use super::packet::*;
use super::store::IldcpStore;
use futures::Future;
use interledger_packet::*;
use interledger_service::*;

pub struct IldcpService<S, T> {
  next: S,
  store: T,
}

impl<S, T> IncomingService for IldcpService<S, T>
where
  S: IncomingService,
  T: IldcpStore,
{
  type Future = BoxedIlpFuture;

  fn handle_request(&mut self, request: IncomingRequest) -> Self::Future {
    if is_ildcp_request(&request.prepare) {
      Box::new(
        self
          .store
          .get_account_details(request.from)
          .map_err(|_| {
            RejectBuilder {
              code: ErrorCode::F02_UNREACHABLE,
              message: b"Account details not found",
              triggered_by: &[],
              data: &[],
            }
            .build()
          })
          .and_then(|account_details| {
            let response = IldcpResponseBuilder {
              client_address: &account_details.client_address[..],
              asset_code: account_details.asset_code.as_str(),
              asset_scale: account_details.asset_scale,
            }
            .build();
            let fulfill = Fulfill::from(response);
            Ok(fulfill)
          }),
      )
    } else {
      Box::new(self.next.handle_request(request))
    }
  }
}
