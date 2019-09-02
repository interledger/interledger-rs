// Adapted from the futures-retry example: https://gitlab.com/mexus/futures-retry/blob/master/examples/tcp-client-complex.rs
use futures::future::Future;
use futures_retry::{ErrorHandler, FutureRetry, RetryPolicy};
use http::StatusCode;
use log::trace;
use reqwest::r#async::Client as HTTPClient;
use serde_json::json;
use std::fmt::Display;
use std::time::Duration;
use url::Url;

// The account creation endpoint set by the engines in [RFC536](https://github.com/interledger/rfcs/pull/536)
static ACCOUNTS_ENDPOINT: &str = "accounts";

pub struct Client {
    timeout_ms: Duration,
    max_retries: usize,
}

impl Client {
    /// Timeout duration is in millisecodns
    pub fn new(timeout_ms: Duration, max_retries: usize) -> Self {
        Client {
            timeout_ms,
            max_retries,
        }
    }

    pub fn create_engine_account<T: Display + Copy>(
        &self,
        engine_url: Url,
        id: T,
    ) -> impl Future<Item = StatusCode, Error = reqwest::Error> {
        let mut se_url = engine_url.clone();
        let timeout = self.timeout_ms;
        se_url
            .path_segments_mut()
            .expect("Invalid settlement engine URL")
            .push(ACCOUNTS_ENDPOINT);
        trace!(
            "Sending account {} creation request to settlement engine: {:?}",
            id,
            se_url.clone()
        );

        // The actual HTTP request which gets made to the engine
        let create_settlement_engine_account = move || {
            let client = HTTPClient::builder().timeout(timeout).build().unwrap();
            client
                .post(se_url.as_ref())
                .json(&json!({"id" : id.to_string()}))
                .send()
                .and_then(move |response| {
                    // If the account is not found on the peer's connector, the
                    // retry logic will not get triggered. When the counterparty
                    // tries to add the account, they will complete the handshake.
                    Ok(response.status())
                })
        };

        FutureRetry::new(
            create_settlement_engine_account,
            IoHandler::new(
                self.max_retries,
                format!("[Engine: {}, Account: {}]", engine_url, id),
            ),
        )
    }
}

/// An I/O handler that counts attempts.
struct IoHandler<D> {
    max_attempts: usize,
    current_attempt: usize,
    display_name: D,
}

impl<D> IoHandler<D> {
    fn new(max_attempts: usize, display_name: D) -> Self {
        IoHandler {
            max_attempts,
            current_attempt: 0,
            display_name,
        }
    }
}

// The error handler trait implements the Retry logic based on the received
// Error Status Code.
impl<D> ErrorHandler<reqwest::Error> for IoHandler<D>
where
    D: ::std::fmt::Display,
{
    type OutError = reqwest::Error;

    fn handle(&mut self, e: reqwest::Error) -> RetryPolicy<reqwest::Error> {
        self.current_attempt += 1;
        if self.current_attempt > self.max_attempts {
            trace!(
                "[{}] All attempts ({}) have been used",
                self.display_name,
                self.max_attempts
            );
            return RetryPolicy::ForwardError(e);
        }
        trace!(
            "[{}] Attempt {}/{} has failed",
            self.display_name,
            self.current_attempt,
            self.max_attempts
        );

        if e.is_client_error() {
            // do not retry 4xx
            RetryPolicy::ForwardError(e)
        } else if e.is_timeout() || e.is_server_error() {
            // Retry timeouts and 5xx every 5 seconds
            RetryPolicy::WaitRetry(Duration::from_secs(5))
        } else {
            // Retry other errors slightly more frequently since they may be
            // related to the engine not having started yet
            RetryPolicy::WaitRetry(Duration::from_secs(1))
        }
    }
}
