use async_trait::async_trait;
use chrono::{DateTime, Duration, Utc};
use hex;
use interledger_packet::{ErrorCode, RejectBuilder};
use interledger_service::*;
use log::error;
use ring::digest::{digest, SHA256};
use std::marker::PhantomData;
use tokio::time::timeout;

/// # Validator Service
///
/// Incoming or Outgoing Service responsible for rejecting timed out
/// requests and checking that fulfillments received match the `execution_condition` from the original `Prepare` packets.
/// Forwards everything else.
#[derive(Clone)]
pub struct ValidatorService<IO, S, A> {
    store: S,
    next: IO,
    account_type: PhantomData<A>,
}

impl<I, S, A> ValidatorService<I, S, A>
where
    I: IncomingService<A>,
    S: AddressStore,
    A: Account,
{
    /// Create an incoming validator service
    /// Forwards incoming requests if not expired, else rejects
    pub fn incoming(store: S, next: I) -> Self {
        ValidatorService {
            store,
            next,
            account_type: PhantomData,
        }
    }
}

impl<O, S, A> ValidatorService<O, S, A>
where
    O: OutgoingService<A>,
    S: AddressStore,
    A: Account,
{
    /// Create an outgoing validator service
    /// If outgoing request is not expired, it checks that the provided fulfillment is a valid preimage to the
    /// prepare packet's fulfillment condition, and if so it forwards it, else rejects
    pub fn outgoing(store: S, next: O) -> Self {
        ValidatorService {
            store,
            next,
            account_type: PhantomData,
        }
    }
}

#[async_trait]
impl<I, S, A> IncomingService<A> for ValidatorService<I, S, A>
where
    I: IncomingService<A> + Send + Sync,
    S: AddressStore + Send + Sync,
    A: Account + Send + Sync,
{
    /// On receiving a request:
    /// 1. If the prepare packet in the request is not expired, forward it, otherwise return a reject
    async fn handle_request(&mut self, request: IncomingRequest<A>) -> IlpResult {
        let expires_at = DateTime::<Utc>::from(request.prepare.expires_at());
        let now = Utc::now();
        if expires_at >= now {
            self.next.handle_request(request).await
        } else {
            error!(
                "Incoming packet expired {}ms ago at {:?} (time now: {:?})",
                now.signed_duration_since(expires_at).num_milliseconds(),
                expires_at.to_rfc3339(),
                expires_at.to_rfc3339(),
            );
            Err(RejectBuilder {
                code: ErrorCode::R00_TRANSFER_TIMED_OUT,
                message: &[],
                triggered_by: Some(&self.store.get_ilp_address()),
                data: &[],
            }
            .build())
        }
    }
}

#[async_trait]
impl<O, S, A> OutgoingService<A> for ValidatorService<O, S, A>
where
    O: OutgoingService<A> + Send + Sync,
    S: AddressStore + Send + Sync,
    A: Account + Send + Sync,
{
    /// On sending a request:
    /// 1. If the outgoing packet has expired, return a reject with the appropriate ErrorCode
    /// 1. Tries to forward the request
    ///     - If no response is received before the prepare packet's expiration, it assumes that the outgoing request has timed out.
    ///     - If no timeout occurred, but still errored it will just return the reject
    ///     - If the forwarding is successful, it should receive a fulfill packet. Depending on if the hash of the fulfillment condition inside the fulfill is a preimage of the condition of the prepare:
    ///         - return the fulfill if it matches
    ///         - otherwise reject
    async fn send_request(&mut self, request: OutgoingRequest<A>) -> IlpResult {
        let mut condition: [u8; 32] = [0; 32];
        condition[..].copy_from_slice(request.prepare.execution_condition()); // why?

        let expires_at = DateTime::<Utc>::from(request.prepare.expires_at());
        let now = Utc::now();
        let time_left = expires_at - now;
        let ilp_address = self.store.get_ilp_address();
        if time_left > Duration::zero() {
            // Result of the future
            let result = timeout(
                time_left.to_std().expect("Time left must be positive"),
                self.next.send_request(request),
            )
            .await;

            let fulfill = match result {
                // If the future completed in time, it returns an IlpResult,
                // which gives us the fulfill packet
                Ok(packet) => packet?,
                // If the future timed out, then it results in an error
                Err(_) => {
                    error!(
                        "Outgoing request timed out after {}ms (expiry was: {})",
                        time_left.num_milliseconds(),
                        expires_at,
                    );
                    return Err(RejectBuilder {
                        code: ErrorCode::R00_TRANSFER_TIMED_OUT,
                        message: &[],
                        triggered_by: Some(&ilp_address),
                        data: &[],
                    }
                    .build());
                }
            };

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
        } else {
            error!(
                "Outgoing packet expired {}ms ago",
                (Duration::zero() - time_left).num_milliseconds(),
            );
            // Already expired
            Err(RejectBuilder {
                code: ErrorCode::R00_TRANSFER_TIMED_OUT,
                message: &[],
                triggered_by: Some(&ilp_address),
                data: &[],
            }
            .build())
        }
    }
}

#[cfg(test)]
use interledger_packet::Address;
#[cfg(test)]
use once_cell::sync::Lazy;
#[cfg(test)]
use std::str::FromStr;
#[cfg(test)]
use uuid::Uuid;
#[cfg(test)]
pub static ALICE: Lazy<Username> = Lazy::new(|| Username::from_str("alice").unwrap());
#[cfg(test)]
pub static EXAMPLE_ADDRESS: Lazy<Address> =
    Lazy::new(|| Address::from_str("example.alice").unwrap());
#[cfg(test)]
#[derive(Clone, Debug)]
struct TestAccount(Uuid);
#[cfg(test)]
impl Account for TestAccount {
    fn id(&self) -> Uuid {
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

    fn ilp_address(&self) -> &Address {
        &EXAMPLE_ADDRESS
    }
}

#[cfg(test)]
#[derive(Clone)]
struct TestStore;

#[cfg(test)]
use interledger_errors::AddressStoreError;

#[cfg(test)]
#[async_trait]
impl AddressStore for TestStore {
    /// Saves the ILP Address in the store's memory and database
    async fn set_ilp_address(&self, _ilp_address: Address) -> Result<(), AddressStoreError> {
        unimplemented!()
    }

    async fn clear_ilp_address(&self) -> Result<(), AddressStoreError> {
        unimplemented!()
    }

    /// Get's the store's ilp address from memory
    fn get_ilp_address(&self) -> Address {
        Address::from_str("example.connector").unwrap()
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

    #[tokio::test]
    async fn lets_through_valid_incoming_packet() {
        let requests = Arc::new(Mutex::new(Vec::new()));
        let requests_clone = requests.clone();
        let mut validator = ValidatorService::incoming(
            TestStore,
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
                from: TestAccount(Uuid::new_v4()),
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
            .await;

        assert_eq!(requests.lock().unwrap().len(), 1);
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn rejects_expired_incoming_packet() {
        let requests = Arc::new(Mutex::new(Vec::new()));
        let requests_clone = requests.clone();
        let mut validator = ValidatorService::incoming(
            TestStore,
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
                from: TestAccount(Uuid::new_v4()),
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
            .await;

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

    #[tokio::test]
    async fn lets_through_valid_outgoing_response() {
        let requests = Arc::new(Mutex::new(Vec::new()));
        let requests_clone = requests.clone();
        let mut validator = ValidatorService::outgoing(
            TestStore,
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
                from: TestAccount(Uuid::new_v4()),
                to: TestAccount(Uuid::new_v4()),
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
            .await;

        assert_eq!(requests.lock().unwrap().len(), 1);
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn returns_reject_instead_of_invalid_fulfillment() {
        let requests = Arc::new(Mutex::new(Vec::new()));
        let requests_clone = requests.clone();
        let mut validator = ValidatorService::outgoing(
            TestStore,
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
                from: TestAccount(Uuid::new_v4()),
                to: TestAccount(Uuid::new_v4()),
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
            .await;

        assert_eq!(requests.lock().unwrap().len(), 1);
        assert!(result.is_err());
        assert_eq!(
            result.unwrap_err().code(),
            ErrorCode::F09_INVALID_PEER_RESPONSE
        );
    }
}
