//! # interledger-http
//!
//! Client and server implementations of the [ILP-Over-HTTP](https://github.com/interledger/rfcs/blob/master/0035-ilp-over-http/0035-ilp-over-http.md) bilateral communication protocol.
//! This protocol is intended primarily for server-to-server communication between peers on the Interledger network.
use bytes::Buf;
use error::*;
use futures::Future;
use interledger_service::{Account, Username};
use serde::de::DeserializeOwned;
use url::Url;
use warp::{self, filters::body::FullBody, Filter, Rejection};

mod client;
mod server;

// So that settlement engines can use errors
pub mod error;

pub use self::client::HttpClientService;
pub use self::server::HttpServer;

pub trait HttpAccount: Account {
    fn get_http_url(&self) -> Option<&Url>;
    fn get_http_auth_token(&self) -> Option<&str>;
}

/// The interface for Stores that can be used with the HttpServerService.
// TODO do we need all of these constraints?
pub trait HttpStore: Clone + Send + Sync + 'static {
    type Account: HttpAccount;

    /// Load account details based on the full HTTP Authorization header
    /// received on the incoming HTTP request.
    fn get_account_from_http_auth(
        &self,
        username: &Username,
        token: &str,
    ) -> Box<dyn Future<Item = Self::Account, Error = ()> + Send>;
}

pub fn deserialize_json<T: DeserializeOwned + Send>(
) -> impl Filter<Extract = (T,), Error = Rejection> + Copy {
    warp::header::exact("content-type", "application/json")
        .and(warp::body::concat())
        .and_then(|buf: FullBody| {
            let deserializer = &mut serde_json::Deserializer::from_slice(&buf.bytes());
            serde_path_to_error::deserialize(deserializer).map_err(|err| {
                warp::reject::custom(JsonDeserializeError {
                    category: err.inner().classify(),
                    detail: err.inner().to_string(),
                    path: err.path().clone(),
                })
            })
        })
}
