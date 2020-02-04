use super::packet::*;
use super::Account;
use async_trait::async_trait;
use interledger_packet::*;
use interledger_service::*;
use log::debug;
use std::marker::PhantomData;

/// A simple service that intercepts incoming ILDCP requests
/// and responds using the information in the Account struct.
#[derive(Clone)]
pub struct IldcpService<I, A> {
    next: I,
    account_type: PhantomData<A>,
}

impl<I, A> IldcpService<I, A>
where
    I: IncomingService<A>,
    A: Account,
{
    pub fn new(next: I) -> Self {
        IldcpService {
            next,
            account_type: PhantomData,
        }
    }
}

#[async_trait]
impl<I, A> IncomingService<A> for IldcpService<I, A>
where
    I: IncomingService<A> + Send,
    A: Account,
{
    async fn handle_request(&mut self, request: IncomingRequest<A>) -> IlpResult {
        if is_ildcp_request(&request.prepare) {
            let from = request.from.ilp_address();
            let builder = IldcpResponseBuilder {
                ilp_address: &from,
                asset_code: request.from.asset_code(),
                asset_scale: request.from.asset_scale(),
            };
            debug!("Responding to query for ildcp info by account: {:?}", from);
            let response = builder.build();
            Ok(Fulfill::from(response))
        } else {
            self.next.handle_request(request).await
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::get_ildcp_info;
    use once_cell::sync::Lazy;
    use std::str::FromStr;
    use uuid::Uuid;

    pub static ALICE: Lazy<Username> = Lazy::new(|| Username::from_str("alice").unwrap());
    pub static EXAMPLE_ADDRESS: Lazy<Address> =
        Lazy::new(|| Address::from_str("example.alice").unwrap());

    #[derive(Clone, Debug, Copy)]
    struct TestAccount;

    impl Account for TestAccount {
        fn id(&self) -> Uuid {
            Uuid::new_v4()
        }

        fn username(&self) -> &Username {
            &ALICE
        }

        fn asset_scale(&self) -> u8 {
            9
        }

        fn asset_code(&self) -> &str {
            "XYZ"
        }

        fn ilp_address(&self) -> &Address {
            &EXAMPLE_ADDRESS
        }
    }

    #[tokio::test]
    async fn handles_request() {
        let from = TestAccount;
        let prepare = IldcpRequest {}.to_prepare();
        let req = IncomingRequest { from, prepare };
        let mut service = IldcpService::new(incoming_service_fn(|_| {
            Err(RejectBuilder {
                code: ErrorCode::F02_UNREACHABLE,
                message: b"No other incoming handler!",
                data: &[],
                triggered_by: None,
            }
            .build())
        }));

        let result = service.handle_request(req).await.unwrap();
        assert_eq!(result.data().len(), 19);

        let ildpc_info = get_ildcp_info(&mut service, from).await.unwrap();
        assert_eq!(ildpc_info.ilp_address(), EXAMPLE_ADDRESS.clone());
        assert_eq!(ildpc_info.asset_code(), b"XYZ");
        assert_eq!(ildpc_info.asset_scale(), 9);
    }
}
