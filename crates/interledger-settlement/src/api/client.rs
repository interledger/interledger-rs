use crate::core::types::{Quantity, SettlementAccount};
use futures::TryFutureExt;
use interledger_service::Account;
use log::{debug, error, trace};
use reqwest::Client;
use serde_json::json;
use uuid::Uuid;

#[derive(Clone)]
pub struct SettlementClient {
    http_client: Client,
}

impl SettlementClient {
    pub fn new() -> Self {
        SettlementClient {
            http_client: Client::new(),
        }
    }

    pub async fn send_settlement<A: SettlementAccount + Account>(
        &self,
        account: A,
        amount: u64,
    ) -> Result<(), ()> {
        if let Some(settlement_engine) = account.settlement_engine_details() {
            let mut settlement_engine_url = settlement_engine.url;
            settlement_engine_url
                .path_segments_mut()
                .expect("Invalid settlement engine URL")
                .push("accounts")
                .push(&account.id().to_string())
                .push("settlements");
            debug!(
                "Sending settlement of amount {} to settlement engine: {}",
                amount, settlement_engine_url
            );
            let settlement_engine_url_clone = settlement_engine_url.clone();
            let idempotency_uuid = Uuid::new_v4().to_hyphenated().to_string();
            let response = self
                .http_client
                .post(settlement_engine_url.as_ref())
                .header("Idempotency-Key", idempotency_uuid)
                .json(&json!(Quantity::new(amount, account.asset_scale())))
                .send()
                .map_err(move |err| {
                    error!(
                        "Error sending settlement command to settlement engine {}: {:?}",
                        settlement_engine_url, err
                    )
                })
                .await?;

            if response.status().is_success() {
                trace!(
                    "Sent settlement of {} to settlement engine: {}",
                    amount,
                    settlement_engine_url_clone
                );
                return Ok(());
            } else {
                error!(
                    "Error sending settlement. Settlement engine responded with HTTP code: {}",
                    response.status()
                );
                return Err(());
            }
        }
        error!("Cannot send settlement for account {} because it does not have the settlement_engine_url and scale configured", account.id());
        Err(())
    }
}

impl Default for SettlementClient {
    fn default() -> Self {
        SettlementClient::new()
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
