use crate::error::ApiError;
use std::error::Error as StdError;
use thiserror::Error;

/// Errors for the ExchangeRateStore
#[derive(Error, Debug)]
#[non_exhaustive]
pub enum ExchangeRateStoreError {
    #[error("{0}")]
    Other(#[from] Box<dyn StdError + Send + 'static>),
    #[error("Pair {from}/{to} not found")]
    PairNotFound { from: String, to: String },
}

impl From<ExchangeRateStoreError> for ApiError {
    fn from(src: ExchangeRateStoreError) -> Self {
        ApiError::internal_server_error().detail(src.to_string())
    }
}

#[cfg(feature = "warp_errors")]
impl From<ExchangeRateStoreError> for warp::Rejection {
    fn from(src: ExchangeRateStoreError) -> Self {
        ApiError::from(src).into()
    }
}
