use super::{AccountStoreError, BtpStoreError, CreateAccountError};
use crate::error::ApiError;
use std::error::Error as StdError;
use thiserror::Error;

/// Errors for the NodeStore
#[derive(Error, Debug)]
#[non_exhaustive]
pub enum NodeStoreError {
    #[error("{0}")]
    Other(#[from] Box<dyn StdError + Send + 'static>),
    #[error("Settlement engine URL loaded was not a valid url: {0}")]
    InvalidEngineUrl(String),
    #[error("account `{0}` was not found")]
    AccountNotFound(String),
    #[error("account `{0}` already exists")]
    AccountExists(String),
    #[error("not all of the given accounts exist")]
    MissingAccounts,
    #[error("invalid account: {0}")]
    InvalidAccount(CreateAccountError),
}

impl From<NodeStoreError> for BtpStoreError {
    fn from(src: NodeStoreError) -> Self {
        match src {
            NodeStoreError::AccountNotFound(s) => BtpStoreError::AccountNotFound(s),
            _ => BtpStoreError::Other(Box::new(src)),
        }
    }
}

impl From<AccountStoreError> for NodeStoreError {
    fn from(src: AccountStoreError) -> Self {
        match src {
            AccountStoreError::AccountNotFound(s) => NodeStoreError::AccountNotFound(s),
            _ => NodeStoreError::Other(Box::new(src)),
        }
    }
}

impl From<NodeStoreError> for ApiError {
    fn from(src: NodeStoreError) -> Self {
        match src {
            NodeStoreError::AccountNotFound(_) => {
                ApiError::account_not_found().detail(src.to_string())
            }
            NodeStoreError::InvalidAccount(_) | NodeStoreError::InvalidEngineUrl(_) => {
                ApiError::bad_request().detail(src.to_string())
            }
            _ => ApiError::internal_server_error().detail(src.to_string()),
        }
    }
}

#[cfg(feature = "warp_errors")]
impl From<NodeStoreError> for warp::Rejection {
    fn from(src: NodeStoreError) -> Self {
        ApiError::from(src).into()
    }
}

#[cfg(feature = "redis_errors")]
use redis::RedisError;
#[cfg(feature = "redis_errors")]
impl From<RedisError> for NodeStoreError {
    fn from(src: RedisError) -> Self {
        NodeStoreError::Other(Box::new(src))
    }
}
