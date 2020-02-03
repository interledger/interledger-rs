use super::{ApiError, NodeStoreError};
use interledger_packet::Address;
use std::error::Error as StdError;
use thiserror::Error;

/// Errors for the AddressStore
#[derive(Error, Debug)]
#[non_exhaustive]
pub enum AddressStoreError {
    #[error("{0}")]
    Other(#[from] Box<dyn StdError + Send>),
    #[error("Could not save address: {0}")]
    SetAddress(Address),
    #[error("Could not save address: {0}")]
    ClearAddress(Address),
}

impl From<NodeStoreError> for AddressStoreError {
    fn from(src: NodeStoreError) -> Self {
        AddressStoreError::Other(Box::new(src))
    }
}

#[cfg(feature = "redis_errors")]
use redis::RedisError;
#[cfg(feature = "redis_errors")]
impl From<RedisError> for AddressStoreError {
    fn from(src: RedisError) -> Self {
        AddressStoreError::Other(Box::new(src))
    }
}

impl From<AddressStoreError> for ApiError {
    fn from(src: AddressStoreError) -> Self {
        // AddressStore erroring is always an internal server error
        ApiError::internal_server_error().detail(src.to_string())
    }
}

#[cfg(feature = "warp_errors")]
impl From<AddressStoreError> for warp::Rejection {
    fn from(src: AddressStoreError) -> Self {
        ApiError::from(src).into()
    }
}
