use crate::{packet::*, RoutingAccount};
use bytes::Bytes;
use futures::{
    future::{err, ok},
    Future,
};
use interledger_packet::*;
use interledger_service::{Account, BoxedIlpFuture, IncomingRequest, IncomingService};
use std::{marker::PhantomData, str};
use tokio_executor::spawn;

pub struct CcpServerService<S, A> {
    ilp_address: Bytes,
    next: S,
    account_type: PhantomData<A>,
}

impl<S, A> CcpServerService<S, A>
where
    S: IncomingService<A>,
    A: Account + RoutingAccount,
{
    // Note the next service will be used both to pass on incoming requests that are not CCP requests
    // as well as send outgoing messages for CCP
    pub fn new(ilp_address: Bytes, next: S) -> Self {
        CcpServerService {
            ilp_address,
            next,
            account_type: PhantomData,
        }
    }

    fn handle_route_control_request(
        &self,
        request: IncomingRequest<A>,
    ) -> impl Future<Item = Fulfill, Error = Reject> {
        if !request.from.should_send_routes() {
            return err(RejectBuilder {
                code: ErrorCode::F00_BAD_REQUEST,
                message: b"We are not configured to send routes to you, sorry",
                triggered_by: &self.ilp_address[..],
                data: &[],
            }
            .build());
        }
        ok(CCP_RESPONSE.clone())
    }

    fn handle_route_update_request(
        &self,
        request: IncomingRequest<A>,
    ) -> impl Future<Item = Fulfill, Error = Reject> {
        if !request.from.should_receive_routes() {
            return err(RejectBuilder {
                code: ErrorCode::F00_BAD_REQUEST,
                message: b"Your route broadcasts are not accepted here",
                triggered_by: &self.ilp_address[..],
                data: &[],
            }
            .build());
        }
        ok(CCP_RESPONSE.clone())
    }
}

impl<S, A> IncomingService<A> for CcpServerService<S, A>
where
    S: IncomingService<A> + 'static,
    A: Account + RoutingAccount + 'static,
{
    type Future = BoxedIlpFuture;

    fn handle_request(&mut self, request: IncomingRequest<A>) -> Self::Future {
        let destination = request.prepare.destination();
        if destination == CCP_CONTROL_DESTINATION {
            Box::new(self.handle_route_control_request(request))
        } else if destination == CCP_UPDATE_DESTINATION {
            Box::new(self.handle_route_update_request(request))
        } else {
            Box::new(self.next.handle_request(request))
        }
    }
}

#[cfg(test)]
mod helpers {
    use super::*;
    use crate::fixtures::*;
    use futures::future::FutureResult;
    use interledger_service::{incoming_service_fn, ServiceFn};

    lazy_static! {
        pub static ref ROUTING_ACCOUNT: TestAccount = TestAccount {
            id: 1,
            send_routes: true,
            receive_routes: true,
        };
        pub static ref NON_ROUTING_ACCOUNT: TestAccount = TestAccount {
            id: 1,
            send_routes: false,
            receive_routes: false,
        };
    }

    #[derive(Clone, Debug, Copy)]
    pub struct TestAccount {
        id: u64,
        receive_routes: bool,
        send_routes: bool,
    }

    impl Account for TestAccount {
        type AccountId = u64;

        fn id(&self) -> u64 {
            self.id
        }
    }

    impl RoutingAccount for TestAccount {
        fn should_receive_routes(&self) -> bool {
            self.receive_routes
        }

        fn should_send_routes(&self) -> bool {
            self.send_routes
        }
    }

    pub fn test_service(
    ) -> CcpServerService<impl IncomingService<TestAccount, Future = BoxedIlpFuture>, TestAccount>
    {
        CcpServerService::new(
            Bytes::from("example.connector"),
            incoming_service_fn(|_request| {
                Box::new(err(RejectBuilder {
                    code: ErrorCode::F02_UNREACHABLE,
                    message: b"No other handler!",
                    data: &[],
                    triggered_by: b"example.connector",
                }
                .build()))
            }),
        )
    }
}

#[cfg(test)]
mod handle_route_control_request {
    use super::helpers::*;
    use super::*;
    use crate::fixtures::*;

    #[test]
    fn handles_valid_request() {
        test_service()
            .handle_request(IncomingRequest {
                prepare: CONTROL_REQUEST.to_prepare(),
                from: *ROUTING_ACCOUNT,
            })
            .wait()
            .unwrap();
    }

    #[test]
    fn rejects_from_non_sending_account() {
        let result = test_service()
            .handle_request(IncomingRequest {
                prepare: CONTROL_REQUEST.to_prepare(),
                from: *NON_ROUTING_ACCOUNT,
            })
            .wait();
        assert!(result.is_err());
        assert_eq!(
            str::from_utf8(result.unwrap_err().message()).unwrap(),
            "We are not configured to send routes to you, sorry"
        );
    }
}

#[cfg(test)]
mod handle_route_update_request {
    use super::helpers::*;
    use super::*;
    use crate::fixtures::*;

    #[test]
    fn handles_valid_request() {
        test_service()
            .handle_request(IncomingRequest {
                prepare: UPDATE_REQUEST_SIMPLE.to_prepare(),
                from: *ROUTING_ACCOUNT,
            })
            .wait()
            .unwrap();
    }

    #[test]
    fn rejects_from_non_receiving_account() {
        let result = test_service()
            .handle_request(IncomingRequest {
                prepare: UPDATE_REQUEST_SIMPLE.to_prepare(),
                from: *NON_ROUTING_ACCOUNT,
            })
            .wait();
        assert!(result.is_err());
        assert_eq!(
            str::from_utf8(result.unwrap_err().message()).unwrap(),
            "Your route broadcasts are not accepted here",
        );
    }
}
