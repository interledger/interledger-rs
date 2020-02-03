//! # interledger-http
//!
//! Client and server implementations of the [ILP-Over-HTTP](https://github.com/interledger/rfcs/blob/master/0035-ilp-over-http/0035-ilp-over-http.md) bilateral communication protocol.
//! This protocol is intended primarily for server-to-server communication between peers on the Interledger network.
use async_trait::async_trait;
use bytes::Bytes;
use interledger_errors::{ApiError, HttpStoreError, JsonDeserializeError};
use interledger_service::{Account, Username};
use mime::Mime;
use secrecy::SecretString;
use serde::de::DeserializeOwned;
use url::Url;
use warp::{self, Filter, Rejection};

/// [ILP over HTTP](https://interledger.org/rfcs/0035-ilp-over-http/) Outgoing Service
mod client;
/// [ILP over HTTP](https://interledger.org/rfcs/0035-ilp-over-http/) API (implemented with [Warp](https://docs.rs/warp/0.2.0/warp/))
mod server;

pub use self::client::HttpClientService;
pub use self::server::HttpServer;

/// Extension trait for [Account](../interledger_service/trait.Account.html) with [ILP over HTTP](https://interledger.org/rfcs/0035-ilp-over-http/) related information
pub trait HttpAccount: Account {
    /// Returns the HTTP URL corresponding to this account
    fn get_http_url(&self) -> Option<&Url>;
    /// Returns the HTTP token which is sent as an HTTP header on each ILP over HTTP request
    fn get_http_auth_token(&self) -> Option<SecretString>;
}

/// The interface for Stores that can be used with the HttpServerService.
// TODO do we need all of these constraints?
#[async_trait]
pub trait HttpStore: Clone + Send + Sync + 'static {
    type Account: HttpAccount;

    /// Load account details based on the full HTTP Authorization header
    /// received on the incoming HTTP request.
    async fn get_account_from_http_auth(
        &self,
        username: &Username,
        token: &str,
    ) -> Result<Self::Account, HttpStoreError>;
}

// TODO: Do we really need this custom deserialization function?
// You'd expect that Serde would be able to handle this.
/// Helper function to deserialize JSON inside Warp
/// The content-type MUST be application/json and if a charset
/// is specified, it MUST be UTF-8
pub fn deserialize_json<T: DeserializeOwned + Send>(
) -> impl Filter<Extract = (T,), Error = Rejection> + Copy {
    warp::header::<String>("content-type")
        .and(warp::body::bytes())
        .and_then(|content_type: String, buf: Bytes| {
            async move {
                let mime_type: Mime = content_type.parse().map_err(|_| {
                    Rejection::from(ApiError::bad_request().detail("Invalid content-type header."))
                })?;
                if mime_type.type_() != mime::APPLICATION_JSON.type_() {
                    return Err(Rejection::from(
                        ApiError::bad_request().detail("Invalid content-type."),
                    ));
                } else if let Some(charset) = mime_type.get_param("charset") {
                    // Charset should be UTF-8
                    // https://tools.ietf.org/html/rfc8259#section-8.1
                    if charset != mime::UTF_8 {
                        return Err(Rejection::from(
                            ApiError::bad_request().detail("Charset should be UTF-8."),
                        ));
                    }
                }

                let deserializer = &mut serde_json::Deserializer::from_slice(&buf);
                serde_path_to_error::deserialize(deserializer).map_err(|err| {
                    warp::reject::custom(JsonDeserializeError {
                        category: err.inner().classify(),
                        detail: err.inner().to_string(),
                        path: err.path().clone(),
                    })
                })
            }
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

    #[tokio::test]
    async fn deserialize_json_header() {
        let json_filter = deserialize_json::<TestJsonStruct>();
        let body_correct = r#"{"string_value": "some string value"}"#;
        let body_incorrect = r#"{"other_key": 0}"#;

        // `content-type` should be provided.
        assert_eq!(
            request().body(body_correct).matches(&json_filter).await,
            false
        );

        // Should accept only "application/json" or "application/json; charset=utf-8"
        assert_eq!(
            request()
                .body(body_correct)
                .header("content-type", "text/plain")
                .matches(&json_filter)
                .await,
            false
        );
        assert_eq!(
            request()
                .body(body_correct)
                .header("content-type", "application/json")
                .matches(&json_filter)
                .await,
            true
        );
        assert_eq!(
            request()
                .body(body_correct)
                .header("content-type", "application/json; charset=ascii")
                .matches(&json_filter)
                .await,
            false
        );
        assert_eq!(
            request()
                .body(body_correct)
                .header("content-type", "application/json; charset=utf-8")
                .matches(&json_filter)
                .await,
            true
        );
        assert_eq!(
            request()
                .body(body_correct)
                .header("content-type", "application/json; charset=UTF-8")
                .matches(&json_filter)
                .await,
            true
        );

        // Should accept only bodies that can be deserialized
        assert_eq!(
            request()
                .body(body_incorrect)
                .header("content-type", "application/json")
                .matches(&json_filter)
                .await,
            false
        );
        assert_eq!(
            request()
                .body(body_incorrect)
                .header("content-type", "application/json; charset=utf-8")
                .matches(&json_filter)
                .await,
            false
        );
    }
}
