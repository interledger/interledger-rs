use super::{error::*, HttpStore};
use bytes::{Bytes, BytesMut};
use futures::future::ok;
use futures::TryFutureExt;
use interledger_packet::{Fulfill, Prepare, Reject};
use interledger_service::Username;
use interledger_service::{IncomingRequest, IncomingService};
use log::error;
use secrecy::{ExposeSecret, SecretString};
use std::convert::TryFrom;
use std::net::SocketAddr;
use warp::{Filter, Rejection};

/// Max message size that is allowed to transfer from a request or a message.
pub const MAX_PACKET_SIZE: u64 = 40000;
pub const BEARER_TOKEN_START: usize = 7;

/// A warp filter that parses incoming ILP-Over-HTTP requests, validates the authorization,
/// and passes the request to an IncomingService handler.
#[derive(Clone)]
pub struct HttpServer<I, S> {
    incoming: I,
    store: S,
}

impl<I, S> HttpServer<I, S>
where
    I: IncomingService<S::Account> + Clone + Send + Sync + 'static,
    S: HttpStore,
{
    pub fn new(incoming: I, store: S) -> Self {
        HttpServer { incoming, store }
    }

    pub fn as_filter(
        &self,
    ) -> impl warp::Filter<Extract = (impl warp::Reply,), Error = warp::Rejection> + Clone + '_
    {
        let self_clone = self.clone();
        warp::post()
            .and(warp::path("accounts"))
            .and(warp::path::param::<Username>())
            .and(warp::path("ilp"))
            .and(warp::path::end())
            .and(warp::header::<SecretString>("authorization"))
            .and(warp::body::content_length_limit(MAX_PACKET_SIZE))
            .and(warp::body::bytes())
            .and_then(move |username, password, body| self.ilp_over_http(username, password, body))
    }

    #[inline]
    async fn get_account(
        &self,
        path_username: &Username,
        password: &SecretString,
    ) -> Result<S::Account, ()> {
        if password.expose_secret().len() < BEARER_TOKEN_START {
            return Err(());
        }
        self.store
            .get_account_from_http_auth(
                &path_username,
                &password.expose_secret()[BEARER_TOKEN_START..],
            )
            .await
    }

    #[inline]
    async fn ilp_over_http(
        &self,
        path_username: Username,
        password: SecretString,
        body: bytes05::Bytes,
    ) -> Result<impl warp::Reply, warp::Rejection> {
        let account = self
            .get_account(&path_username, &password)
            .map_err(|_| -> Rejection {
                error!("Invalid authorization provided for user: {}", path_username);
                ApiError::unauthorized().into()
            })
            .await?;

        let buffer = bytes::BytesMut::from(body.as_ref());
        if let Ok(prepare) = Prepare::try_from(buffer) {
            let result = self.incoming
                .handle_request(IncomingRequest {
                    from: account,
                    prepare,
                })
                .await; // Awaiting this future causes the error.

            let bytes: bytes05::BytesMut = match result {
                Ok(fulfill) => fulfill.into(),
                Err(reject) => reject.into(),
            };

            Ok(warp::http::Response::builder()
                .header("Content-Type", "application/octet-stream")
                .status(200)
                // .body(bytes.freeze()) // TODO: bring this back
                .body(bytes05::Bytes::new())
                .unwrap())
        } else {
            error!("Body was not a valid Prepare packet");
            Err(Rejection::from(ApiError::invalid_ilp_packet()))
        }
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
    use bytes::{Bytes, BytesMut};
    use futures::future::ok;
    use http::Response;
    use interledger_packet::{Address, ErrorCode, PrepareBuilder, RejectBuilder};
    use interledger_service::{incoming_service_fn, Account};
    use lazy_static::lazy_static;
    use secrecy::SecretString;
    use std::convert::TryInto;
    use std::str::FromStr;
    use std::time::SystemTime;
    use url::Url;
    use uuid::Uuid;

    lazy_static! {
        static ref USERNAME: Username = Username::from_str("alice").unwrap();
        static ref ILP_ADDRESS: Address = Address::from_str("example.alice").unwrap();
        pub static ref PREPARE_BYTES: BytesMut = PrepareBuilder {
            amount: 0,
            destination: ILP_ADDRESS.clone(),
            expires_at: SystemTime::now(),
            execution_condition: &[0; 32],
            data: &[],
        }
        .build()
        .try_into()
        .unwrap();
    }
    const AUTH_PASSWORD: &str = "password";

    fn api_call<F>(
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
    }

    #[test]
    fn new_api_test() {
        let store = TestStore;
        let incoming = incoming_service_fn(|_request| {
            Box::new(err(RejectBuilder {
                code: ErrorCode::F02_UNREACHABLE,
                message: b"No other incoming handler!",
                data: &[],
                triggered_by: None,
            }
            .build()))
        });
        let api = HttpServer::new(incoming, store)
            .as_filter()
            .recover(default_rejection_handler);

        // Fails with overloaded token
        let resp = api_call(
            &api,
            "/accounts/alice/ilp",
            &format!("{}:{}", USERNAME.to_string(), AUTH_PASSWORD),
        );
        assert_eq!(resp.status().as_u16(), 401);

        // Works with just the password
        let resp = api_call(&api, "/accounts/alice/ilp", AUTH_PASSWORD);
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

    impl HttpStore for TestStore {
        type Account = TestAccount;
        fn get_account_from_http_auth(
            &self,
            username: &Username,
            token: &str,
        ) -> Box<dyn Future<Item = Self::Account, Error = ()> + Send> {
            if username == &*USERNAME && token == AUTH_PASSWORD {
                Box::new(ok(TestAccount))
            } else {
                Box::new(err(()))
            }
        }
    }
}
