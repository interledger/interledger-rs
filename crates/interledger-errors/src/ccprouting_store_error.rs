use super::{AccountStoreError, NodeStoreError};
use crate::error::ApiError;
use std::error::Error as StdError;
use thiserror::Error;

/// Errors for the CcpRoutingStore
#[derive(Error, Debug)]
#[non_exhaustive]
pub enum CcpRoutingStoreError {
    #[error("{0}")]
    Other(#[from] Box<dyn StdError + Send + 'static>),
}

impl From<AccountStoreError> for CcpRoutingStoreError {
    fn from(src: AccountStoreError) -> Self {
        CcpRoutingStoreError::Other(Box::new(src))
    }
}

impl From<NodeStoreError> for CcpRoutingStoreError {
    fn from(src: NodeStoreError) -> Self {
        CcpRoutingStoreError::Other(Box::new(src))
    }
}

impl From<CcpRoutingStoreError> for ApiError {
    fn from(src: CcpRoutingStoreError) -> Self {
        match src {
            _ => ApiError::method_not_allowed(),
        }
    }
}

#[cfg(feature = "warp_errors")]
impl From<CcpRoutingStoreError> for warp::Rejection {
    fn from(src: CcpRoutingStoreError) -> Self {
        ApiError::from(src).into()
    }
}

#[cfg(feature = "redis_errors")]
use redis::RedisError;

#[cfg(feature = "redis_errors")]
impl From<RedisError> for CcpRoutingStoreError {
    fn from(src: RedisError) -> CcpRoutingStoreError {
        CcpRoutingStoreError::Other(Box::new(src))
    }
}
