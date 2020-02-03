use crate::error::ApiError;
use std::error::Error as StdError;
use thiserror::Error;

/// Errors for the LeftoversStore
#[derive(Error, Debug)]
#[non_exhaustive]
pub enum LeftoversStoreError {
    #[error("{0}")]
    Other(#[from] Box<dyn StdError + Send + 'static>),
}

impl From<LeftoversStoreError> for ApiError {
    fn from(src: LeftoversStoreError) -> Self {
        match src {
            _ => ApiError::method_not_allowed(),
        }
    }
}

#[cfg(feature = "warp_errors")]
impl From<LeftoversStoreError> for warp::Rejection {
    fn from(src: LeftoversStoreError) -> Self {
        ApiError::from(src).into()
    }
}

/// Errors for the IdempotentStore
#[derive(Error, Debug)]
#[non_exhaustive]
pub enum IdempotentStoreError {
    #[error("{0}")]
    Other(#[from] Box<dyn StdError + Send + 'static>),
}

impl From<IdempotentStoreError> for ApiError {
    fn from(src: IdempotentStoreError) -> Self {
        match src {
            _ => ApiError::method_not_allowed(),
        }
    }
}

#[cfg(feature = "warp_errors")]
impl From<IdempotentStoreError> for warp::Rejection {
    fn from(src: IdempotentStoreError) -> Self {
        ApiError::from(src).into()
    }
}

/// Errors for the SettlementStore
#[derive(Error, Debug)]
#[non_exhaustive]
pub enum SettlementStoreError {
    #[error("{0}")]
    Other(#[from] Box<dyn StdError + Send + 'static>),
    #[error("could not update balance for incoming settlement")]
    BalanceUpdateFailure,
    #[error("could not refund settlement")]
    RefundFailure,
}

impl From<SettlementStoreError> for ApiError {
    fn from(src: SettlementStoreError) -> Self {
        match src {
            _ => ApiError::method_not_allowed(),
        }
    }
}

impl From<LeftoversStoreError> for SettlementStoreError {
    fn from(src: LeftoversStoreError) -> SettlementStoreError {
        SettlementStoreError::Other(Box::new(src))
    }
}

#[cfg(feature = "warp_errors")]
impl From<SettlementStoreError> for warp::Rejection {
    fn from(src: SettlementStoreError) -> Self {
        ApiError::from(src).into()
    }
}

#[cfg(feature = "redis_errors")]
use redis::RedisError;

#[cfg(feature = "redis_errors")]
impl From<RedisError> for SettlementStoreError {
    fn from(src: RedisError) -> SettlementStoreError {
        SettlementStoreError::Other(Box::new(src))
    }
}

#[cfg(feature = "redis_errors")]
impl From<RedisError> for LeftoversStoreError {
    fn from(src: RedisError) -> LeftoversStoreError {
        LeftoversStoreError::Other(Box::new(src))
    }
}

#[cfg(feature = "redis_errors")]
impl From<RedisError> for IdempotentStoreError {
    fn from(src: RedisError) -> IdempotentStoreError {
        IdempotentStoreError::Other(Box::new(src))
    }
}
