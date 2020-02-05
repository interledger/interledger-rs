use crate::core::types::Quantity;
use futures_retry::{FutureRetry, RetryPolicy};
use log::{debug, trace};
use reqwest::Client;
use serde_json::json;
use std::time::Duration;
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

    pub async fn create_engine_account(&self, id: Uuid, engine_url: Url) -> Response {
        FutureRetry::new(
            move || self.create_engine_account_once(id.clone(), engine_url.clone()),
            handle_error,
        )
        .await
    }

    pub async fn send_settlement(
        &self,
        id: Uuid,
        engine_url: Url,
        amount: u64,
        asset_scale: u8,
    ) -> Response {
        FutureRetry::new(
            move || self.send_settlement_once(id, engine_url.clone(), amount, asset_scale),
            handle_error,
        )
        .await
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

    /// Sends a request to create a new account on the settlement engine

    /// Sends a settlement request to the node's settlement engine for the provided account and amount
    ///
    /// # Errors
    /// 1. Account has no engine configured
    /// 1. HTTP request to engine failed from node side
    /// 1. HTTP response from engine was an error
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
            .push("accounts")
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

fn handle_error(e: reqwest::Error) -> RetryPolicy<reqwest::Error> {
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

impl Default for SettlementClient {
    fn default() -> Self {
        SettlementClient::new(DEFAULT_HTTP_TIMEOUT, MAX_RETRIES)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::fixtures::TEST_ACCOUNT_0;
    use crate::api::test_helpers::mock_settlement;
    use mockito::Matcher;

    #[tokio::test]
    async fn settlement_ok() {
        let m = mock_settlement(200)
            .match_header("Idempotency-Key", Matcher::Any)
            .create();
        let client = SettlementClient::new();

        let ret = client.send_settlement(TEST_ACCOUNT_0.clone(), 100).await;

        m.assert();
        assert!(ret.is_ok());
    }

    #[tokio::test]
    async fn engine_rejects() {
        let m = mock_settlement(500)
            .match_header("Idempotency-Key", Matcher::Any)
            .create();
        let client = SettlementClient::new();

        let ret = client.send_settlement(TEST_ACCOUNT_0.clone(), 100).await;

        m.assert();
        assert!(ret.is_err());
    }

    #[tokio::test]
    async fn account_does_not_have_settlement_engine() {
        let m = mock_settlement(200)
            .expect(0)
            .match_header("Idempotency-Key", Matcher::Any)
            .create();
        let client = SettlementClient::new();

        let mut acc = TEST_ACCOUNT_0.clone();
        acc.no_details = true; // Hide the settlement engine data from the account
        let ret = client.send_settlement(acc, 100).await;

        m.assert();
        assert!(ret.is_err());
    }
}
