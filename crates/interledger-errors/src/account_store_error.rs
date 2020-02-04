use super::BtpStoreError;
use crate::error::ApiError;
use std::error::Error as StdError;
use thiserror::Error;

/// Errors for the AccountStore
#[derive(Error, Debug)]
#[non_exhaustive]
pub enum AccountStoreError {
    #[error("{0}")]
    Other(#[from] Box<dyn StdError + Send + 'static>),
    #[error("account `{0}` was not found")]
    AccountNotFound(String),
    #[error("account `{0}` already exists")]
    AccountExists(String),
    #[error("wrong account length (expected {expected}, got {actual})")]
    WrongLength { expected: usize, actual: usize },
}

impl From<AccountStoreError> for BtpStoreError {
    fn from(src: AccountStoreError) -> Self {
        match src {
            AccountStoreError::AccountNotFound(s) => BtpStoreError::AccountNotFound(s),
            _ => BtpStoreError::Other(Box::new(src)),
        }
    }
}

impl From<AccountStoreError> for ApiError {
    fn from(src: AccountStoreError) -> Self {
        match src {
            AccountStoreError::AccountNotFound(_) => {
                ApiError::account_not_found().detail(src.to_string())
            }
            AccountStoreError::AccountExists(_) => ApiError::conflict().detail(src.to_string()),
            _ => ApiError::internal_server_error().detail(src.to_string()),
        }
    }
}

#[cfg(feature = "warp_errors")]
impl From<AccountStoreError> for warp::Rejection {
    fn from(src: AccountStoreError) -> Self {
        ApiError::from(src).into()
    }
}

#[cfg(feature = "redis_errors")]
use redis::RedisError;

#[cfg(feature = "redis_errors")]
impl From<RedisError> for AccountStoreError {
    fn from(err: RedisError) -> Self {
        AccountStoreError::Other(Box::new(err))
    }
}
