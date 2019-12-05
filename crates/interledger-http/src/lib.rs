//! # interledger-http
//!
//! Client and server implementations of the [ILP-Over-HTTP](https://github.com/interledger/rfcs/blob/master/0035-ilp-over-http/0035-ilp-over-http.md) bilateral communication protocol.
//! This protocol is intended primarily for server-to-server communication between peers on the Interledger network.
use bytes::Buf;
use error::*;
use futures::Future;
use interledger_service::{Account, Username};
use mime::Mime;
use secrecy::SecretString;
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
    fn get_http_auth_token(&self) -> Option<SecretString>;
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
    warp::header::<String>("content-type")
        .and(warp::body::concat())
        .and_then(|content_type: String, buf: FullBody| {
            let mime_type: Mime = content_type.parse().map_err::<Rejection, _>(|_| {
                error::ApiError::bad_request()
                    .detail("Invalid content-type header.")
                    .into()
            })?;
            if mime_type.type_() != mime::APPLICATION_JSON.type_() {
                return Err(error::ApiError::bad_request()
                    .detail("Invalid content-type.")
                    .into());
            } else if let Some(charset) = mime_type.get_param("charset") {
                // Charset should be UTF-8
                // https://tools.ietf.org/html/rfc8259#section-8.1
                if charset != mime::UTF_8 {
                    return Err(error::ApiError::bad_request()
                        .detail("Charset should be UTF-8.")
                        .into());
                }
            }

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

#[cfg(test)]
mod tests {
    use super::deserialize_json;
    use serde::Deserialize;
    use warp::test::request;

    #[derive(Deserialize, Clone)]
    struct TestJsonStruct {
        string_value: String,
    }

    #[test]
    fn deserialize_json_header() {
        let json_filter = deserialize_json::<TestJsonStruct>();
        let body_correct = r#"{"string_value": "some string value"}"#;
        let body_incorrect = r#"{"other_key": 0}"#;

        // `content-type` should be provided.
        assert_eq!(request().body(body_correct).matches(&json_filter), false);

        // Should accept only "application/json" or "application/json; charset=utf-8"
        assert_eq!(
            request()
                .body(body_correct)
                .header("content-type", "text/plain")
                .matches(&json_filter),
            false
        );
        assert_eq!(
            request()
                .body(body_correct)
                .header("content-type", "application/json")
                .matches(&json_filter),
            true
        );
        assert_eq!(
            request()
                .body(body_correct)
                .header("content-type", "application/json; charset=ascii")
                .matches(&json_filter),
            false
        );
        assert_eq!(
            request()
                .body(body_correct)
                .header("content-type", "application/json; charset=utf-8")
                .matches(&json_filter),
            true
        );
        assert_eq!(
            request()
                .body(body_correct)
                .header("content-type", "application/json; charset=UTF-8")
                .matches(&json_filter),
            true
        );

        // Should accept only bodies that can be deserialized
        assert_eq!(
            request()
                .body(body_incorrect)
                .header("content-type", "application/json")
                .matches(&json_filter),
            false
        );
        assert_eq!(
            request()
                .body(body_incorrect)
                .header("content-type", "application/json; charset=utf-8")
                .matches(&json_filter),
            false
        );
    }
}
