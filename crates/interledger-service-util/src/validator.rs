use futures::{future::err, Future};
use interledger_packet::{ErrorCode, RejectBuilder};
use interledger_service::*;
use ring::digest::{digest, SHA256};
use std::{marker::PhantomData, time::SystemTime};
use tokio::prelude::FutureExt;

#[derive(Clone)]
pub struct ValidatorService<S, A> {
    next: S,
    account_type: PhantomData<A>,
}

impl<S, A> ValidatorService<S, A>
where
    S: IncomingService<A>,
    A: Account,
{
    pub fn incoming(next: S) -> Self {
        ValidatorService {
            next,
            account_type: PhantomData,
        }
    }
}

impl<S, A> ValidatorService<S, A>
where
    S: OutgoingService<A>,
    A: Account,
{
    pub fn outgoing(next: S) -> Self {
        ValidatorService {
            next,
            account_type: PhantomData,
        }
    }
}

impl<S, A> IncomingService<A> for ValidatorService<S, A>
where
    S: IncomingService<A>,
    A: Account,
{
    type Future = BoxedIlpFuture;

    fn handle_request(&mut self, request: IncomingRequest<A>) -> Self::Future {
        if request.prepare.expires_at() <= SystemTime::now() {
            Box::new(self.next.handle_request(request))
        } else {
            let result = Box::new(err(RejectBuilder {
                code: ErrorCode::R00_TRANSFER_TIMED_OUT,
                message: &[],
                triggered_by: &[],
                data: &[],
            }
            .build()));
            Box::new(result)
        }
    }
}

impl<S, A> OutgoingService<A> for ValidatorService<S, A>
where
    S: OutgoingService<A>,
    A: Account,
{
    type Future = BoxedIlpFuture;

    fn send_request(&mut self, request: OutgoingRequest<A>) -> Self::Future {
        let mut condition: [u8; 32] = [0; 32];
        condition[..].copy_from_slice(request.prepare.execution_condition());

        if let Ok(timeout) = request
            .prepare
            .expires_at()
            .duration_since(SystemTime::now())
        {
            Box::new(
                self.next
                    .send_request(request)
                    .timeout(timeout)
                    .map_err(|err| {
                        // If the error was caused by the timer, into_inner will return None
                        if let Some(reject) = err.into_inner() {
                            reject
                        } else {
                            RejectBuilder {
                                code: ErrorCode::R00_TRANSFER_TIMED_OUT,
                                message: &[],
                                triggered_by: &[],
                                data: &[],
                            }
                            .build()
                        }
                    })
                    .and_then(move |fulfill| {
                        if digest(&SHA256, &fulfill.data()).as_ref() == condition {
                            Ok(fulfill)
                        } else {
                            Err(RejectBuilder {
                                code: ErrorCode::F09_INVALID_PEER_RESPONSE,
                                message: b"Fulfillment did not match condition",
                                triggered_by: &[],
                                data: &[],
                            }
                            .build())
                        }
                    }),
            )
        } else {
            // Already expired
            Box::new(err(RejectBuilder {
                code: ErrorCode::R00_TRANSFER_TIMED_OUT,
                message: &[],
                triggered_by: &[],
                data: &[],
            }
            .build()))
        }
    }
}
