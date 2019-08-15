use super::packet::*;
use futures::future::ok;
use interledger_packet::*;
use interledger_service::*;
use log::debug;
use std::marker::PhantomData;

/// A simple service that intercepts incoming ILDCP requests
/// and responds using the information in the Account struct.
#[derive(Clone)]
pub struct IldcpService<I, A> {
    ilp_address: Address,
    next: I,
    account_type: PhantomData<A>,
}

impl<I, A> IldcpService<I, A>
where
    I: IncomingService<A> + Clone + Send + Sync + 'static,
    A: Account + Clone + Send + Sync + 'static,
{
    /// The provided ilp address will be used as a base which will be prepended
    /// to the account's username, unless the account has specified an ilp
    /// themselves as an override.
    pub fn new(ilp_address: Address, next: I) -> Self {
        IldcpService {
            ilp_address,
            next,
            account_type: PhantomData,
        }
    }
}

impl<I, A> IncomingService<A> for IldcpService<I, A>
where
    I: IncomingService<A> + Clone + Send + Sync + 'static,
    A: Account + Clone + Send + Sync + 'static,
{
    type Future = BoxedIlpFuture;

    fn handle_request(&mut self, request: IncomingRequest<A>) -> Self::Future {
        if is_ildcp_request(&request.prepare) {
            let ilp_address = self.ilp_address.clone();
            let full_address = if let Some(ilp_address) = request.from.ilp_address() {
                // if they have set an ilp_address, that means they are
                // overriding with their own and we should not append their username to
                // our node's address
                ilp_address.clone()
            } else {
                // todo: handle the with_suffix exception
                ilp_address
                    .with_suffix(request.from.username().as_ref())
                    .unwrap()
            };

            let builder = IldcpResponseBuilder {
                client_address: &full_address,
                asset_code: request.from.asset_code(),
                asset_scale: request.from.asset_scale(),
            };
            debug!(
                "Responding to query for ildcp info of use {} with address: {:?}",
                request.from.username(),
                full_address
            );
            let response = builder.build();
            let fulfill = Fulfill::from(response);
            Box::new(ok(fulfill))
        } else {
            Box::new(self.next.handle_request(request))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_helpers::*;
    use bytes::Bytes;
    use futures::future::Future;
    use std::convert::TryFrom;
    use std::str::FromStr;
    use std::time::SystemTime;

    #[test]
    fn appends_username() {
        let mut service = test_service();
        let result = service
            .handle_request(IncomingRequest {
                from: USERNAME_ACC.clone(),
                prepare: PrepareBuilder {
                    destination: ILDCP_DESTINATION.clone(),
                    amount: 100,
                    execution_condition: &PEER_PROTOCOL_CONDITION,
                    expires_at: SystemTime::UNIX_EPOCH,
                    data: &[],
                }
                .build(),
            })
            .wait();

        let fulfill: Fulfill = result.unwrap();
        let response = IldcpResponse::try_from(Bytes::from(fulfill.data())).unwrap();
        assert_eq!(
            Address::from_str("example.connector.ausername").unwrap(),
            response.client_address()
        );
    }

    #[test]
    fn overrides_with_ilp_address() {
        let mut service = test_service();
        let result = service
            .handle_request(IncomingRequest {
                from: ILPADDR_ACC.clone(),
                prepare: PrepareBuilder {
                    destination: ILDCP_DESTINATION.clone(),
                    amount: 100,
                    execution_condition: &PEER_PROTOCOL_CONDITION,
                    expires_at: SystemTime::UNIX_EPOCH,
                    data: &[],
                }
                .build(),
            })
            .wait();

        let fulfill: Fulfill = result.unwrap();
        let response = IldcpResponse::try_from(Bytes::from(fulfill.data())).unwrap();
        assert_eq!(
            Address::from_str("example.account").unwrap(),
            response.client_address()
        );
    }

}
