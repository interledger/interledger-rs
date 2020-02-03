use crate::error::ApiError;
use interledger_packet::ParseError;
use std::error::Error as StdError;
use thiserror::Error;
use url::ParseError as UrlParseError;

/// Errors which can happen when creating an account
#[derive(Error, Debug)]
#[non_exhaustive]
pub enum CreateAccountError {
    #[error("{0}")]
    Other(#[from] Box<dyn StdError + Send + 'static>),
    #[error("the provided suffix is not valid: {0}")]
    InvalidSuffix(ParseError),
    #[error("the provided http url is not valid: {0}")]
    InvalidHttpUrl(UrlParseError),
    #[error("the provided btp url is not valid: {0}")]
    InvalidBtpUrl(UrlParseError),
    #[error("the provided routing relation is not valid: {0}")]
    InvalidRoutingRelation(String),
    #[error("the provided value for parameter `{0}` was too large")]
    ParamTooLarge(String),
}

impl From<CreateAccountError> for ApiError {
    fn from(src: CreateAccountError) -> Self {
        ApiError::bad_request().detail(src.to_string())
    }
}

#[cfg(feature = "warp_errors")]
impl From<CreateAccountError> for warp::Rejection {
    fn from(src: CreateAccountError) -> Self {
        ApiError::from(src).into()
    }
}

#[cfg(feature = "redis_errors")]
use redis::RedisError;

#[cfg(feature = "redis_errors")]
impl From<RedisError> for CreateAccountError {
    fn from(err: RedisError) -> Self {
        CreateAccountError::Other(Box::new(err))
    }
}
