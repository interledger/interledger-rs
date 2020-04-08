use super::HttpStore;
use bytes::{Bytes, BytesMut};
use interledger_errors::ApiError;
use interledger_packet::Prepare;
use interledger_service::Username;
use interledger_service::{IncomingRequest, IncomingService};
use secrecy::{ExposeSecret, SecretString};
use std::convert::TryFrom;
use std::net::SocketAddr;
use tracing::error;
use warp::{Filter, Rejection};

/// Max message size that is allowed to transfer from a request or a message.
pub const MAX_PACKET_SIZE: u64 = 40000;
/// The offset after which the bearer token should be in an ILP over HTTP request
/// e.g. in `token = "Bearer: MyAuthToken"`, `MyAuthToken` can be taken via token[BEARER_TOKEN_START..]
pub const BEARER_TOKEN_START: usize = 7;

/// A warp filter that parses incoming ILP-Over-HTTP requests, validates the authorization,
/// and passes the request to an IncomingService handler.
#[derive(Clone)]
pub struct HttpServer<I, S> {
    /// The next [incoming service](../interledger_service/trait.IncomingService.html)
    incoming: I,
    /// A store which implements [`HttpStore`](trait.HttpStore.html)
    store: S,
}

#[inline]
/// Returns the account which matches the provided username/password combination
/// from the store, or returns an error if the account was not found or if the
/// credentials were incorrect
async fn get_account<S>(
    store: S,
    path_username: &Username,
    password: &SecretString,
) -> Result<S::Account, ApiError>
where
    S: HttpStore,
{
    if password.expose_secret().len() < BEARER_TOKEN_START {
        return Err(ApiError::unauthorized().detail("provided token was not a bearer token"));
    }
    Ok(store
        .get_account_from_http_auth(
            &path_username,
            &password.expose_secret()[BEARER_TOKEN_START..],
        )
        .await?)
}

#[inline]
/// Implements ILP over HTTP. If account authentication is valid
/// and the provided packet can be parsed as a
/// [Prepare](../../interledger_packet/struct.Prepare.html) packet,
/// then it is forwarded to the next incoming service which will return
/// an Ok result if the response is a [Fulfill](../../interledger_packet/struct.Fulfill.html).
///
/// # Errors
/// 1. Unauthorized account if invalid credentials are provided
/// 1. The provided `body` could not be parsed as a Prepare packet
/// 1. A Reject packet was returned by the next incoming service
async fn ilp_over_http<S, I>(
    path_username: Username,
    password: SecretString,
    body: Bytes,
    store: S,
    incoming: I,
) -> Result<impl warp::Reply, warp::Rejection>
where
    S: HttpStore,
    I: IncomingService<S::Account> + Clone,
{
    let mut incoming = incoming.clone();
    let account = get_account(store, &path_username, &password).await?;

    let buffer = bytes::BytesMut::from(body.as_ref());
    if let Ok(prepare) = Prepare::try_from(buffer) {
        let result = incoming
            .handle_request(IncomingRequest {
                from: account,
                prepare,
            })
            .await;

        let bytes: BytesMut = match result {
            Ok(fulfill) => fulfill.into(),
            Err(reject) => reject.into(),
        };

        Ok(warp::http::Response::builder()
            .header("Content-Type", "application/octet-stream")
            .status(200)
            .body(bytes.freeze()) // TODO: bring this back
            .unwrap())
    } else {
        error!("Body was not a valid Prepare packet");
        Err(Rejection::from(ApiError::invalid_ilp_packet()))
    }
}

impl<I, S> HttpServer<I, S>
where
    I: IncomingService<S::Account> + Clone + Send + Sync,
    S: HttpStore + Clone,
{
    pub fn new(incoming: I, store: S) -> Self {
        HttpServer { incoming, store }
    }

    /// Returns a Warp filter which exposes per-account endpoints for [ILP over HTTP](https://interledger.org/rfcs/0035-ilp-over-http/).
    /// The endpoint is /accounts/:username/ilp.
    pub fn as_filter(
        &self,
    ) -> impl warp::Filter<Extract = (impl warp::Reply,), Error = warp::Rejection> + Clone {
        let store = self.store.clone();
        let incoming = self.incoming.clone();
        let with_store = warp::any().map(move || store.clone()).boxed();
        let with_incoming = warp::any().map(move || incoming.clone());
        warp::post()
            .and(warp::path("accounts"))
            .and(warp::path::param::<Username>())
            .and(warp::path("ilp"))
            .and(warp::path::end())
            .and(warp::header::<SecretString>("authorization"))
            .and(warp::body::content_length_limit(MAX_PACKET_SIZE))
            .and(warp::body::bytes())
            .and(with_store)
            .and(with_incoming)
            .and_then(ilp_over_http)
    }

    // Do we really need to bind self to static?
    pub async fn bind(&'static self, addr: SocketAddr) {
        let filter = self.as_filter();
        warp::serve(filter).run(addr).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::HttpAccount;
    use async_trait::async_trait;
    use bytes::BytesMut;
    use http::Response;
    use interledger_errors::{default_rejection_handler, HttpStoreError};
    use interledger_packet::{Address, ErrorCode, PrepareBuilder, RejectBuilder};
    use interledger_service::{incoming_service_fn, Account};
    use once_cell::sync::Lazy;
    use secrecy::SecretString;
    use std::convert::TryInto;
    use std::str::FromStr;
    use std::time::SystemTime;
    use url::Url;
    use uuid::Uuid;

    static USERNAME: Lazy<Username> = Lazy::new(|| Username::from_str("alice").unwrap());
    static ILP_ADDRESS: Lazy<Address> = Lazy::new(|| Address::from_str("example.alice").unwrap());
    pub static PREPARE_BYTES: Lazy<BytesMut> = Lazy::new(|| {
        PrepareBuilder {
            amount: 0,
            destination: ILP_ADDRESS.clone(),
            expires_at: SystemTime::now(),
            execution_condition: &[0; 32],
            data: &[],
        }
        .build()
        .try_into()
        .unwrap()
    });

    const AUTH_PASSWORD: &str = "password";

    async fn api_call<F>(
        api: &F,
        endpoint: &str, // /ilp or /accounts/:username/ilp
        auth: &str,     // simple bearer or overloaded username+password
    ) -> Response<Bytes>
    where
        F: warp::Filter + 'static,
        F::Extract: warp::Reply,
    {
        warp::test::request()
            .method("POST")
            .path(endpoint)
            .header("Authorization", format!("Bearer {}", auth))
            .header("Content-length", 1000)
            .body(PREPARE_BYTES.clone())
            .reply(api)
            .await
    }

    #[tokio::test]
    async fn new_api_test() {
        let store = TestStore;
        let incoming = incoming_service_fn(|_request| {
            Err(RejectBuilder {
                code: ErrorCode::F02_UNREACHABLE,
                message: b"No other incoming handler!",
                data: &[],
                triggered_by: None,
            }
            .build())
        });
        let api = HttpServer::new(incoming, store)
            .as_filter()
            .recover(default_rejection_handler);

        // Fails with overloaded token
        let resp = api_call(
            &api,
            "/accounts/alice/ilp",
            &format!("{}:{}", USERNAME.to_string(), AUTH_PASSWORD),
        )
        .await;
        assert_eq!(resp.status().as_u16(), 401);

        // Works with just the password
        let resp = api_call(&api, "/accounts/alice/ilp", AUTH_PASSWORD).await;
        assert_eq!(resp.status().as_u16(), 200);
    }

    #[derive(Debug, Clone)]
    struct TestAccount;
    impl Account for TestAccount {
        fn id(&self) -> Uuid {
            Uuid::new_v4()
        }

        fn username(&self) -> &Username {
            &USERNAME
        }
        fn ilp_address(&self) -> &Address {
            &ILP_ADDRESS
        }

        fn asset_scale(&self) -> u8 {
            9
        }
        fn asset_code(&self) -> &str {
            "XYZ"
        }
    }

    impl HttpAccount for TestAccount {
        fn get_http_auth_token(&self) -> Option<SecretString> {
            unimplemented!()
        }

        fn get_http_url(&self) -> Option<&Url> {
            unimplemented!()
        }
    }

    #[derive(Debug, Clone)]
    struct TestStore;

    #[async_trait]
    impl HttpStore for TestStore {
        type Account = TestAccount;

        async fn get_account_from_http_auth(
            &self,
            username: &Username,
            token: &str,
        ) -> Result<Self::Account, HttpStoreError> {
            if username == &*USERNAME && token == AUTH_PASSWORD {
                Ok(TestAccount)
            } else {
                Err(HttpStoreError::Unauthorized(username.to_string()))
            }
        }
    }
}
