use crate::error::ApiError;
use std::error::Error as StdError;
use thiserror::Error;

/// Errors for the BtpStore
#[derive(Error, Debug)]
#[non_exhaustive]
pub enum BtpStoreError {
    #[error("{0}")]
    Other(#[from] Box<dyn StdError + Send + 'static>),
    #[error("account `{0}` was not found")]
    AccountNotFound(String),
    #[error("account `{0}` is not authorized for this action")]
    Unauthorized(String),
}

impl From<BtpStoreError> for ApiError {
    fn from(src: BtpStoreError) -> Self {
        match src {
            BtpStoreError::AccountNotFound(_) => {
                ApiError::account_not_found().detail(src.to_string())
            }
            BtpStoreError::Unauthorized(_) => ApiError::unauthorized().detail(src.to_string()),
            _ => ApiError::internal_server_error().detail(src.to_string()),
        }
    }
}

#[cfg(feature = "warp_errors")]
impl From<BtpStoreError> for warp::Rejection {
    fn from(src: BtpStoreError) -> Self {
        ApiError::from(src).into()
    }
}

#[cfg(feature = "redis_errors")]
use redis::RedisError;

#[cfg(feature = "redis_errors")]
impl From<RedisError> for BtpStoreError {
    fn from(src: RedisError) -> BtpStoreError {
        BtpStoreError::Other(Box::new(src))
    }
}
