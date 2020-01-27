#[cfg(feature = "google_pubsub")]
use base64;
use chrono::Utc;
use futures::{compat::Future01CompatExt, Future, TryFutureExt};
use interledger::{
    packet::Address,
    service::{Account, IlpResult, OutgoingRequest, OutgoingService, Username},
};
use parking_lot::Mutex;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, sync::Arc};
use tokio::spawn;
use tracing::{error, info};
use yup_oauth2::{service_account_key_from_file, GetToken, ServiceAccountAccess};

static TOKEN_SCOPES: Option<&str> = Some("https://www.googleapis.com/auth/pubsub");

/// Configuration for the Google PubSub packet publisher
#[derive(Deserialize, Clone, Debug)]
pub struct PubsubConfig {
    /// Path to the Service Account Key JSON file.
    /// You can obtain this file by logging into [console.cloud.google.com](https://console.cloud.google.com/)
    service_account_credentials: String,
    project_id: String,
    topic: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct PubsubMessage {
    message_id: Option<String>,
    data: Option<String>,
    attributes: Option<HashMap<String, String>>,
    publish_time: Option<String>,
}

#[derive(Serialize)]
struct PubsubRequest {
    messages: Vec<PubsubMessage>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct PacketRecord {
    prev_hop_account: Username,
    prev_hop_asset_code: String,
    prev_hop_asset_scale: u8,
    prev_hop_amount: u64,
    next_hop_account: Username,
    next_hop_asset_code: String,
    next_hop_asset_scale: u8,
    next_hop_amount: u64,
    destination_ilp_address: Address,
    fulfillment: String,
    timestamp: String,
}

use std::pin::Pin;
type BoxedIlpFuture = Box<dyn Future<Output = IlpResult> + Send + 'static>;

/// Create an Interledger service wrapper that publishes records
/// of fulfilled packets to Google Cloud PubSub.
///
/// This is an experimental feature that may be removed in the future.
pub fn create_google_pubsub_wrapper<A: Account + 'static>(
    config: Option<PubsubConfig>,
) -> impl Fn(OutgoingRequest<A>, Box<dyn OutgoingService<A> + Send>) -> Pin<BoxedIlpFuture> + Clone
{
    // If Google credentials were passed in, create an HTTP client and
    // OAuth2 client that will automatically fetch and cache access tokens
    let utilities = if let Some(config) = config {
        let key = service_account_key_from_file(config.service_account_credentials.as_str())
            .expect("Unable to load Google Cloud credentials from file");
        let access = ServiceAccountAccess::new(key);
        // This needs to be wrapped in a Mutex because the .token()
        // method takes a mutable reference to self and we want to
        // reuse the same fetcher so that it caches the tokens
        let token_fetcher = Arc::new(Mutex::new(access.build()));

        // TODO make sure the client uses HTTP/2
        let client = Client::new();
        let api_endpoint = Arc::new(format!(
            "https://pubsub.googleapis.com/v1/projects/{}/topics/{}:publish",
            config.project_id, config.topic
        ));
        info!("Fulfilled packets will be submitted to Google Cloud Pubsub (project ID: {}, topic: {})", config.project_id, config.topic);
        Some((client, api_endpoint, token_fetcher))
    } else {
        None
    };

    move |request: OutgoingRequest<A>,
          mut next: Box<dyn OutgoingService<A> + Send>|
          -> Pin<BoxedIlpFuture> {
        let (client, api_endpoint, token_fetcher) = if let Some(utilities) = utilities.clone() {
            utilities
        } else {
            return Box::pin(async move { next.send_request(request).await });
        };

        // Just pass the request on if no Google Pubsub details were configured
        let prev_hop_account = request.from.username().clone();
        let prev_hop_asset_code = request.from.asset_code().to_string();
        let prev_hop_asset_scale = request.from.asset_scale();
        let prev_hop_amount = request.original_amount;
        let next_hop_account = request.to.username().clone();
        let next_hop_asset_code = request.to.asset_code().to_string();
        let next_hop_asset_scale = request.to.asset_scale();
        let next_hop_amount = request.prepare.amount();
        let destination_ilp_address = request.prepare.destination();

        Box::pin(async move {
            let result = next.send_request(request).await;

            // Only fulfilled packets are published for now
            if let Ok(fulfill) = result.clone() {
                let fulfillment = base64::encode(fulfill.fulfillment());
                let get_token_future =
                    token_fetcher
                        .lock()
                        .token(TOKEN_SCOPES)
                        .compat()
                        .map_err(|err| {
                            error!("Error fetching OAuth token for Google PubSub: {:?}", err)
                        });

                // Spawn a task to submit the packet to PubSub so we
                // don't block returning the fulfillment
                // Note this means that if there is a problem submitting the
                // packet record to PubSub, it will only log an error
                spawn(async move {
                    let token = get_token_future.await?;
                    let record = PacketRecord {
                        prev_hop_account,
                        prev_hop_asset_code,
                        prev_hop_asset_scale,
                        prev_hop_amount,
                        next_hop_account,
                        next_hop_asset_code,
                        next_hop_asset_scale,
                        next_hop_amount,
                        destination_ilp_address,
                        fulfillment,
                        timestamp: Utc::now().to_rfc3339(),
                    };
                    let data = base64::encode(&serde_json::to_string(&record).unwrap());
                    let res = client
                        .post(api_endpoint.as_str())
                        .bearer_auth(token.access_token)
                        .json(&PubsubRequest {
                            messages: vec![PubsubMessage {
                                // TODO should there be an ID?
                                message_id: None,
                                data: Some(data),
                                attributes: None,
                                publish_time: None,
                            }],
                        })
                        .send()
                        .map_err(|err| {
                            error!("Error sending packet details to Google PubSub: {:?}", err)
                        })
                        .await?;

                    // Log the error
                    if !res.status().is_success() {
                        let status = res.status();
                        let body = res
                            .text()
                            .map_err(|err| error!("Error getting response body: {:?}", err))
                            .await?;
                        error!(
                            %status,
                            "Error sending packet details to Google PubSub: {}",
                            body
                        );
                    }

                    Ok::<(), ()>(())
                });
            }

            result
        })
    }
}
