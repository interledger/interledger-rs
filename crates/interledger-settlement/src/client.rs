use super::SettlementAccount;
use futures::{
    future::{err, Either},
    Future,
};
use interledger_ildcp::IldcpAccount;
use reqwest::r#async::Client;
use serde::Serialize;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SendSettlement {
    account_id: String,
    amount: String,
}

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

    pub fn send_settlement<A: SettlementAccount + IldcpAccount>(
        &self,
        account: A,
        amount: u64,
    ) -> impl Future<Item = (), Error = ()> {
        if let Some(settlement_engine) = account.settlement_engine_details() {
            let mut settlement_engine_url = settlement_engine.url;
            let amount = if settlement_engine.asset_scale >= account.asset_scale() {
                amount
                    * 10u64.pow(u32::from(
                        settlement_engine.asset_scale - account.asset_scale(),
                    ))
            } else {
                amount
                    / 10u64.pow(u32::from(
                        account.asset_scale() - settlement_engine.asset_scale,
                    ))
            };

            settlement_engine_url
                .path_segments_mut()
                .expect("Invalid settlement engine URL")
                .push("sendMoney");
            trace!(
                "Sending settlement of amount {} to settlement engine: {}",
                amount,
                settlement_engine_url
            );
            // TODO add auth
            // TOOD add id and make settlement call idempotent
            let settlement_engine_url_clone = settlement_engine_url.clone();
            return Either::A(self.http_client.post(settlement_engine_url.clone())
                .json(&SendSettlement {
                    account_id: account.id().to_string(),
                    amount: format!("{}", amount),
                })
                .send()
                .map_err(move |err| error!("Error sending settlement command to settlement engine {}: {:?}", settlement_engine_url, err))
                .and_then(move |response| {
                    if response.status().is_success() {
                        trace!("Sent settlement of {} to settlement engine: {}", amount, settlement_engine_url_clone);
                        Ok(())
                    } else {
                        error!("Error sending settlement. Settlement engine responded with HTTP code: {}", response.status());
                        Err(())
                    }
                }));
        }
        error!("Cannot send settlement for account {} because it does not have the settlement_engine_url and scale configured", account.id());
        Either::B(err(()))
    }
}

impl Default for SettlementClient {
    fn default() -> Self {
        SettlementClient::new()
    }
}
