use chrono::{DateTime, Duration, Utc};
use futures::{future::err, Future};
use hex;
use interledger_packet::{Address, ErrorCode, RejectBuilder};
use interledger_service::*;
use log::error;
use ring::digest::{digest, SHA256};
use std::marker::PhantomData;
use tokio::prelude::FutureExt;

/// # Validator Service
///
/// Incoming or Outgoing Service responsible for rejecting timed out
/// requests and checking that fulfillments received match the `execution_condition` from the original `Prepare` packets.
/// Forwards everything else.
///
#[derive(Clone)]
pub struct ValidatorService<IO, A> {
    ilp_address: Address,
    next: IO,
    account_type: PhantomData<A>,
}

impl<I, A> ValidatorService<I, A>
where
    I: IncomingService<A>,
    A: Account,
{
    pub fn incoming(ilp_address: Address, next: I) -> Self {
        ValidatorService {
            ilp_address,
            next,
            account_type: PhantomData,
        }
    }
}

impl<O, A> ValidatorService<O, A>
where
    O: OutgoingService<A>,
    A: Account,
{
    pub fn outgoing(ilp_address: Address, next: O) -> Self {
        ValidatorService {
            ilp_address,
            next,
            account_type: PhantomData,
        }
    }
}

impl<I, A> IncomingService<A> for ValidatorService<I, A>
where
    I: IncomingService<A>,
    A: Account,
{
    type Future = BoxedIlpFuture;

    /// On receiving a request:
    /// 1. If the prepare packet in the request is not expired, forward it, otherwise return a reject
    fn handle_request(&mut self, request: IncomingRequest<A>) -> Self::Future {
        let expires_at = DateTime::<Utc>::from(request.prepare.expires_at());
        let now = Utc::now();
        if expires_at >= now {
            Box::new(self.next.handle_request(request))
        } else {
            error!(
                "Incoming packet expired {}ms ago at {:?} (time now: {:?})",
                now.signed_duration_since(expires_at).num_milliseconds(),
                expires_at.to_rfc3339(),
                expires_at.to_rfc3339(),
            );
            let result = Box::new(err(RejectBuilder {
                code: ErrorCode::R00_TRANSFER_TIMED_OUT,
                message: &[],
                triggered_by: Some(&self.ilp_address),
                data: &[],
            }
            .build()));
            Box::new(result)
        }
    }
}

impl<O, A> OutgoingService<A> for ValidatorService<O, A>
where
    O: OutgoingService<A>,
    A: Account,
{
    type Future = BoxedIlpFuture;

    /// On sending a request:
    /// 1. If the outgoing packet has expired, return a reject with the appropriate ErrorCode
    /// 1. Tries to forward the request
    ///     - If no response is received before the prepare packet's expiration, it assumes that the outgoing request has timed out.
    ///     - If no timeout occurred, but still errored it will just return the reject
    ///     - If the forwarding is successful, it should receive a fulfill packet. Depending on if the hash of the fulfillment condition inside the fulfill is a preimage of the condition of the prepare:
    ///         - return the fulfill if it matches
    ///         - otherwise reject
    fn send_request(&mut self, request: OutgoingRequest<A>) -> Self::Future {
        let mut condition: [u8; 32] = [0; 32];
        condition[..].copy_from_slice(request.prepare.execution_condition()); // why?

        let expires_at = DateTime::<Utc>::from(request.prepare.expires_at());
        let now = Utc::now();
        let time_left = expires_at - now;
        let ilp_address = self.ilp_address.clone();
        let ilp_address_clone = ilp_address.clone();
        if time_left > Duration::zero() {
            Box::new(
                self.next
                    .send_request(request)
                    .timeout(time_left.to_std().expect("Time left must be positive"))
                    .map_err(move |err| {
                        // If the error was caused by the timer, into_inner will return None
                        if let Some(reject) = err.into_inner() {
                            reject
                        } else {
                            error!(
                                "Outgoing request timed out after {}ms (expiry was: {})",
                                time_left.num_milliseconds(),
                                expires_at,
                            );
                            RejectBuilder {
                                code: ErrorCode::R00_TRANSFER_TIMED_OUT,
                                message: &[],
                                triggered_by: Some(&ilp_address_clone),
                                data: &[],
                            }
                            .build()
                        }
                    })
                    .and_then(move |fulfill| {
                        let generated_condition = digest(&SHA256, fulfill.fulfillment());
                        if generated_condition.as_ref() == condition {
                            Ok(fulfill)
                        } else {
                            error!("Fulfillment did not match condition. Fulfillment: {}, hash: {}, actual condition: {}", hex::encode(fulfill.fulfillment()), hex::encode(generated_condition), hex::encode(condition));
                            Err(RejectBuilder {
                                code: ErrorCode::F09_INVALID_PEER_RESPONSE,
                                message: b"Fulfillment did not match condition",
                                triggered_by: Some(&ilp_address),
                                data: &[],
                            }
                            .build())
                        }
                    }),
            )
        } else {
            error!(
                "Outgoing packet expired {}ms ago",
                (Duration::zero() - time_left).num_milliseconds(),
            );
            // Already expired
            Box::new(err(RejectBuilder {
                code: ErrorCode::R00_TRANSFER_TIMED_OUT,
                message: &[],
                triggered_by: Some(&ilp_address),
                data: &[],
            }
            .build()))
        }
    }
}

#[cfg(test)]
use lazy_static::lazy_static;
#[cfg(test)]
use std::str::FromStr;
#[cfg(test)]
lazy_static! {
    pub static ref ALICE: Username = Username::from_str("alice").unwrap();
    pub static ref EXAMPLE_ADDRESS: Address = Address::from_str("example.alice").unwrap();
}
#[cfg(test)]
#[derive(Clone, Debug)]
struct TestAccount(u64);
#[cfg(test)]
impl Account for TestAccount {
    type AccountId = u64;

    fn id(&self) -> u64 {
        self.0
    }

    fn username(&self) -> &Username {
        &ALICE
    }

    fn asset_code(&self) -> &str {
        "XYZ"
    }

    // All connector accounts use asset scale = 9.
    fn asset_scale(&self) -> u8 {
        9
    }

    fn client_address(&self) -> &Address {
        &EXAMPLE_ADDRESS
    }
}

#[cfg(test)]
mod incoming {
    use super::*;
    use interledger_packet::*;
    use interledger_service::incoming_service_fn;
    use std::{
        sync::{Arc, Mutex},
        time::{Duration, SystemTime},
    };

    #[test]
    fn lets_through_valid_incoming_packet() {
        let requests = Arc::new(Mutex::new(Vec::new()));
        let requests_clone = requests.clone();
        let mut validator = ValidatorService::incoming(
            Address::from_str("example.connector").unwrap(),
            incoming_service_fn(move |request| {
                requests_clone.lock().unwrap().push(request);
                Ok(FulfillBuilder {
                    fulfillment: &[0; 32],
                    data: b"test data",
                }
                .build())
            }),
        );
        let result = validator
            .handle_request(IncomingRequest {
                from: TestAccount(0),
                prepare: PrepareBuilder {
                    destination: Address::from_str("example.destination").unwrap(),
                    amount: 100,
                    expires_at: SystemTime::now() + Duration::from_secs(30),
                    execution_condition: &[
                        102, 104, 122, 173, 248, 98, 189, 119, 108, 143, 193, 139, 142, 159, 142,
                        32, 8, 151, 20, 133, 110, 226, 51, 179, 144, 42, 89, 29, 13, 95, 41, 37,
                    ],
                    data: b"test data",
                }
                .build(),
            })
            .wait();

        assert_eq!(requests.lock().unwrap().len(), 1);
        assert!(result.is_ok());
    }

    #[test]
    fn rejects_expired_incoming_packet() {
        let requests = Arc::new(Mutex::new(Vec::new()));
        let requests_clone = requests.clone();
        let mut validator = ValidatorService::incoming(
            Address::from_str("example.connector").unwrap(),
            incoming_service_fn(move |request| {
                requests_clone.lock().unwrap().push(request);
                Ok(FulfillBuilder {
                    fulfillment: &[0; 32],
                    data: b"test data",
                }
                .build())
            }),
        );
        let result = validator
            .handle_request(IncomingRequest {
                from: TestAccount(0),
                prepare: PrepareBuilder {
                    destination: Address::from_str("example.destination").unwrap(),
                    amount: 100,
                    expires_at: SystemTime::now() - Duration::from_secs(30),
                    execution_condition: &[
                        102, 104, 122, 173, 248, 98, 189, 119, 108, 143, 193, 139, 142, 159, 142,
                        32, 8, 151, 20, 133, 110, 226, 51, 179, 144, 42, 89, 29, 13, 95, 41, 37,
                    ],
                    data: b"test data",
                }
                .build(),
            })
            .wait();

        assert!(requests.lock().unwrap().is_empty());
        assert!(result.is_err());
        assert_eq!(
            result.unwrap_err().code(),
            ErrorCode::R00_TRANSFER_TIMED_OUT
        );
    }
}

#[cfg(test)]
mod outgoing {
    use super::*;
    use interledger_packet::*;
    use std::str::FromStr;
    use std::{
        sync::{Arc, Mutex},
        time::{Duration, SystemTime},
    };

    #[test]
    fn lets_through_valid_outgoing_response() {
        let requests = Arc::new(Mutex::new(Vec::new()));
        let requests_clone = requests.clone();
        let mut validator = ValidatorService::outgoing(
            Address::from_str("example.connector").unwrap(),
            outgoing_service_fn(move |request| {
                requests_clone.lock().unwrap().push(request);
                Ok(FulfillBuilder {
                    fulfillment: &[0; 32],
                    data: b"test data",
                }
                .build())
            }),
        );
        let result = validator
            .send_request(OutgoingRequest {
                from: TestAccount(1),
                to: TestAccount(2),
                original_amount: 100,
                prepare: PrepareBuilder {
                    destination: Address::from_str("example.destination").unwrap(),
                    amount: 100,
                    expires_at: SystemTime::now() + Duration::from_secs(30),
                    execution_condition: &[
                        102, 104, 122, 173, 248, 98, 189, 119, 108, 143, 193, 139, 142, 159, 142,
                        32, 8, 151, 20, 133, 110, 226, 51, 179, 144, 42, 89, 29, 13, 95, 41, 37,
                    ],
                    data: b"test data",
                }
                .build(),
            })
            .wait();

        assert_eq!(requests.lock().unwrap().len(), 1);
        assert!(result.is_ok());
    }

    #[test]
    fn returns_reject_instead_of_invalid_fulfillment() {
        let requests = Arc::new(Mutex::new(Vec::new()));
        let requests_clone = requests.clone();
        let mut validator = ValidatorService::outgoing(
            Address::from_str("example.connector").unwrap(),
            outgoing_service_fn(move |request| {
                requests_clone.lock().unwrap().push(request);
                Ok(FulfillBuilder {
                    fulfillment: &[1; 32],
                    data: b"test data",
                }
                .build())
            }),
        );
        let result = validator
            .send_request(OutgoingRequest {
                from: TestAccount(1),
                to: TestAccount(2),
                original_amount: 100,
                prepare: PrepareBuilder {
                    destination: Address::from_str("example.destination").unwrap(),
                    amount: 100,
                    expires_at: SystemTime::now() + Duration::from_secs(30),
                    execution_condition: &[
                        102, 104, 122, 173, 248, 98, 189, 119, 108, 143, 193, 139, 142, 159, 142,
                        32, 8, 151, 20, 133, 110, 226, 51, 179, 144, 42, 89, 29, 13, 95, 41, 37,
                    ],
                    data: b"test data",
                }
                .build(),
            })
            .wait();

        assert_eq!(requests.lock().unwrap().len(), 1);
        assert!(result.is_err());
        assert_eq!(
            result.unwrap_err().code(),
            ErrorCode::F09_INVALID_PEER_RESPONSE
        );
    }
}
