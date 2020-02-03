use crate::error::ApiError;
use std::error::Error as StdError;
use thiserror::Error;

/// Errors for the BalanceStore
#[derive(Error, Debug)]
#[non_exhaustive]
pub enum BalanceStoreError {
    #[error("{0}")]
    Other(#[from] Box<dyn StdError + Send + 'static>),
}

impl From<BalanceStoreError> for ApiError {
    fn from(src: BalanceStoreError) -> Self {
        ApiError::internal_server_error().detail(src.to_string())
    }
}

#[cfg(feature = "warp_errors")]
impl From<BalanceStoreError> for warp::Rejection {
    fn from(src: BalanceStoreError) -> Self {
        ApiError::from(src).into()
    }
}

#[cfg(feature = "redis_errors")]
use redis::RedisError;

#[cfg(feature = "redis_errors")]
impl From<RedisError> for BalanceStoreError {
    fn from(src: RedisError) -> BalanceStoreError {
        BalanceStoreError::Other(Box::new(src))
    }
}
