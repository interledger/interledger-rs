use super::{AccountStoreError, NodeStoreError};
use crate::error::ApiError;
use std::error::Error as StdError;
use thiserror::Error;

/// Errors for the RouteManagerStore
#[derive(Error, Debug)]
#[non_exhaustive]
pub enum RouteManagerStoreError {
    #[error("{0}")]
    Other(#[from] Box<dyn StdError + Send + 'static>),
}

impl From<AccountStoreError> for RouteManagerStoreError {
    fn from(src: AccountStoreError) -> Self {
        RouteManagerStoreError::Other(Box::new(src))
    }
}

impl From<NodeStoreError> for RouteManagerStoreError {
    fn from(src: NodeStoreError) -> Self {
        RouteManagerStoreError::Other(Box::new(src))
    }
}

impl From<RouteManagerStoreError> for ApiError {
    fn from(src: RouteManagerStoreError) -> Self {
        match src {
            _ => ApiError::method_not_allowed(),
        }
    }
}

#[cfg(feature = "warp_errors")]
impl From<RouteManagerStoreError> for warp::Rejection {
    fn from(src: RouteManagerStoreError) -> Self {
        ApiError::from(src).into()
    }
}

#[cfg(feature = "redis_errors")]
use redis::RedisError;

#[cfg(feature = "redis_errors")]
impl From<RedisError> for RouteManagerStoreError {
    fn from(src: RedisError) -> RouteManagerStoreError {
        RouteManagerStoreError::Other(Box::new(src))
    }
}
