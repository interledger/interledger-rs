use crate::core::types::Quantity;
use futures_retry::{ErrorHandler, FutureRetry, RetryPolicy};
use reqwest::Client;
use serde_json::json;
use std::time::Duration;
use tracing::{debug, trace};
use url::Url;
use uuid::Uuid;

type Response = Result<reqwest::Response, reqwest::Error>;

// The account creation endpoint set by the engines in the [RFC](https://github.com/interledger/rfcs/pull/536)
static ACCOUNTS_ENDPOINT: &str = "accounts";
const MAX_RETRIES: usize = 10;
const DEFAULT_HTTP_TIMEOUT: Duration = Duration::from_millis(5000);

/// Helper struct to execute settlements
#[derive(Clone)]
pub struct SettlementClient {
    /// Asynchronous reqwest client
    client: Client,
    max_retries: usize,
}

impl SettlementClient {
    /// Simple constructor
    pub fn new(timeout: Duration, max_retries: usize) -> Self {
        SettlementClient {
            client: Client::builder().timeout(timeout).build().unwrap(),
            max_retries,
        }
    }

    /// Sends an idempotent account creation request to the engine (will retry if it fails)
    /// This is done by sending a POST to /accounts with the provided `id` as the request's body
    pub async fn create_engine_account(&self, id: Uuid, engine_url: Url) -> Response {
        FutureRetry::new(
            move || self.create_engine_account_once(id, engine_url.clone()),
            RequestErrorHandler::new(self.max_retries),
        )
        .await
        .map(|(response, _attempts)| response)
        .map_err(|(err, _attempts)| err)
    }

    /// Sends a message to the engine (will retry idempotently if it fails) which will get forwarded to the peer's engine
    /// This is done by sending a POST to /accounts/:id/messages with the provided `message`
    /// as the request's body
    pub async fn send_message(&self, id: Uuid, engine_url: Url, message: Vec<u8>) -> Response {
        FutureRetry::new(
            move || self.send_message_once(id, engine_url.clone(), message.clone()),
            RequestErrorHandler::new(self.max_retries),
        )
        .await
        .map(|(response, _attempts)| response)
        .map_err(|(err, _attempts)| err)
    }

    async fn send_message_once(&self, id: Uuid, engine_url: Url, message: Vec<u8>) -> Response {
        // The `Prepare` packet's data was sent by the peer's settlement
        // engine so we assume it is in a format that our settlement engine
        // will understand
        // format.
        let mut settlement_engine_url = engine_url;
        settlement_engine_url
            .path_segments_mut()
            .expect("Invalid settlement engine URL")
            .push("accounts")
            .push(&id.to_string())
            .push("messages");
        let idempotency_uuid = uuid::Uuid::new_v4().to_hyphenated().to_string();
        self.client
            .post(settlement_engine_url.as_ref())
            .header("Content-Type", "application/octet-stream")
            .header("Idempotency-Key", idempotency_uuid.clone())
            .body(message.clone())
            .send()
            .await
    }

    /// Sends an idempotent settlement request to the engine (will retry if it fails)
    /// This is done by sending a POST to /accounts/:id/settlements with the provided `amount` and `asset_scale`
    /// as the request's body
    pub async fn send_settlement(
        &self,
        id: Uuid,
        engine_url: Url,
        amount: u64,
        asset_scale: u8,
    ) -> Response {
        FutureRetry::new(
            move || self.send_settlement_once(id, engine_url.clone(), amount, asset_scale),
            RequestErrorHandler::new(self.max_retries),
        )
        .await
        .map(|(response, _attempts)| response)
        .map_err(|(err, _attempts)| err)
    }

    async fn create_engine_account_once(&self, id: Uuid, engine_url: Url) -> Response {
        let mut se_url = engine_url;
        // $URL/accounts
        se_url
            .path_segments_mut()
            .expect("Invalid settlement engine URL")
            .push(ACCOUNTS_ENDPOINT);
        trace!(
            "Sending account {} creation request to settlement engine: {:?}",
            id,
            se_url.clone()
        );

        Ok(self
            .client
            .post(se_url.as_ref())
            .json(&json!({ "id": id.to_string() }))
            .send()
            .await?)
    }

    pub async fn send_settlement_once(
        &self,
        id: Uuid,
        engine_url: Url,
        amount: u64,
        asset_scale: u8,
    ) -> Response {
        let mut settlement_engine_url = engine_url;

        // $URL/accounts/:account_id/settlements
        settlement_engine_url
            .path_segments_mut()
            .expect("Invalid settlement engine URL")
            .push(ACCOUNTS_ENDPOINT)
            .push(&id.to_string())
            .push("settlements");
        debug!(
            "Sending settlement of amount {} to settlement engine: {}",
            amount, settlement_engine_url
        );

        // Mark the request as idempotent
        let idempotency_uuid = Uuid::new_v4().to_hyphenated().to_string();

        // Make the POST request future
        let response = self
            .client
            .post(settlement_engine_url.as_ref())
            .header("Idempotency-Key", idempotency_uuid)
            .json(&json!(Quantity::new(amount, asset_scale)))
            .send()
            .await?;

        Ok(response.error_for_status()?)
    }
}

struct RequestErrorHandler {
    max_attempts: usize,
}

impl RequestErrorHandler {
    fn new(max_attempts: usize) -> Self {
        RequestErrorHandler { max_attempts }
    }
}

impl ErrorHandler<reqwest::Error> for RequestErrorHandler {
    type OutError = reqwest::Error;

    /// Handler of errors for the retry logic
    fn handle(&mut self, attempt: usize, e: reqwest::Error) -> RetryPolicy<reqwest::Error> {
        if attempt == self.max_attempts {
            return RetryPolicy::ForwardError(e);
        }
        if e.is_timeout() {
            RetryPolicy::WaitRetry(Duration::from_secs(5))
        } else if let Some(status) = e.status() {
            if status.is_client_error() {
                // do not retry 4xx
                RetryPolicy::ForwardError(e)
            } else if status.is_server_error() {
                // Retry 5xx every 5 seconds
                RetryPolicy::WaitRetry(Duration::from_secs(5))
            } else {
                // Otherwise just retry every second
                RetryPolicy::WaitRetry(Duration::from_secs(1))
            }
        } else {
            // Retry other errors slightly more frequently since they may be
            // related to the engine not having started yet
            RetryPolicy::WaitRetry(Duration::from_secs(1))
        }
    }
}

impl Default for SettlementClient {
    fn default() -> Self {
        SettlementClient::new(DEFAULT_HTTP_TIMEOUT, MAX_RETRIES)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mockito::{mock, Matcher};
    use once_cell::sync::Lazy;

    pub static SETTLEMENT_API: Lazy<Matcher> = Lazy::new(|| {
        Matcher::Regex(r"^/accounts/[0-9A-Fa-f]{8}-[0-9A-Fa-f]{4}-[0-9A-Fa-f]{4}-[0-9A-Fa-f]{4}-[0-9A-Fa-f]{12}/settlements$".to_string())
    });

    fn mock_settlement(status_code: usize) -> mockito::Mock {
        mock("POST", SETTLEMENT_API.clone())
            // The settlement API receives json data
            .match_header("Content-Type", "application/json")
            .with_status(status_code)
    }

    #[tokio::test]
    async fn settlement_ok() {
        let m = mock_settlement(200)
            .match_header("Idempotency-Key", Matcher::Any)
            .create();
        let client = SettlementClient::default();

        let ret = client
            .send_settlement(
                Uuid::new_v4(),
                "http://localhost:1234".parse().unwrap(),
                100,
                6,
            )
            .await;

        m.assert();
        assert!(ret.is_ok());
    }

    #[tokio::test]
    async fn engine_rejects() {
        let m = mock_settlement(500)
            .match_header("Idempotency-Key", Matcher::Any)
            .create()
            .expect(2); // It will hit it twice because it will retry
        let client = SettlementClient::new(Duration::from_secs(1), 1);

        let ret = client
            .send_settlement(
                Uuid::new_v4(),
                "http://localhost:1234".parse().unwrap(),
                100,
                6,
            )
            .await;

        m.assert();
        assert!(ret.is_err());
    }
}
