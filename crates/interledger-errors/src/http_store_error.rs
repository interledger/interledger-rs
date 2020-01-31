use crate::error::ApiError;
use std::error::Error as StdError;
use thiserror::Error;

/// Errors for the HttpStore
#[derive(Error, Debug)]
pub enum HttpStoreError {
    #[error("{0}")]
    Other(#[from] Box<dyn StdError + Send + 'static>),
    #[error("account with username/id `{0}` was not found")]
    AccountNotFound(String),
    #[error("account with id `{0}` is not authorized for this action")]
    Unauthorized(String),
}

impl From<HttpStoreError> for ApiError {
    fn from(src: HttpStoreError) -> Self {
        match src {
            HttpStoreError::AccountNotFound(_) => {
                ApiError::account_not_found().detail(src.to_string())
            }
            HttpStoreError::Unauthorized(_) => ApiError::unauthorized().detail(src.to_string()),
            _ => ApiError::internal_server_error().detail(src.to_string()),
        }
    }
}

#[cfg(feature = "warp_errors")]
impl From<HttpStoreError> for warp::Rejection {
    fn from(src: HttpStoreError) -> Self {
        ApiError::from(src).into()
    }
}

#[cfg(feature = "redis_errors")]
use redis::RedisError;

#[cfg(feature = "redis_errors")]
impl From<RedisError> for HttpStoreError {
    fn from(src: RedisError) -> HttpStoreError {
        HttpStoreError::Other(Box::new(src))
    }
}
