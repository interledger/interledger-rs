use super::SettlementAccount;
use bytes::Bytes;
use futures::{
    future::{err, Either},
    Future, Stream,
};
use interledger_packet::{ErrorCode, FulfillBuilder, RejectBuilder};
use interledger_service::{BoxedIlpFuture, IncomingRequest, IncomingService};
use reqwest::r#async::Client;
use serde_json::{self, Value};
use std::marker::PhantomData;

const PEER_FULFILLMENT: [u8; 32] = [0; 32];

#[derive(Clone)]
pub struct SettlementMessageService<S, A> {
    ilp_address: Bytes,
    next: S,
    http_client: Client,
    account_type: PhantomData<A>,
}

impl<S, A> SettlementMessageService<S, A>
where
    S: IncomingService<A>,
    A: SettlementAccount,
{
    pub fn new(ilp_address: Bytes, next: S) -> Self {
        SettlementMessageService {
            next,
            ilp_address,
            http_client: Client::new(),
            account_type: PhantomData,
        }
    }
}

impl<S, A> IncomingService<A> for SettlementMessageService<S, A>
where
    S: IncomingService<A>,
    A: SettlementAccount,
{
    type Future = BoxedIlpFuture;

    fn handle_request(&mut self, request: IncomingRequest<A>) -> Self::Future {
        // Only handle the request if the destination address matches the ILP address
        // of the settlement engine being used for this account
        if let Some(settlement_engine_details) = request.from.settlement_engine_details() {
            if request.prepare.destination() == settlement_engine_details.ilp_address.as_ref() {
                let ilp_address = self.ilp_address.clone();
                let mut settlement_engine_url = settlement_engine_details.url;

                match serde_json::from_slice(request.prepare.data()) {
                    Ok(Value::Object(mut message)) => {
                        message.insert(
                            "accountId".to_string(),
                            Value::String(request.from.id().to_string()),
                        );
                        // TODO add auth
                        settlement_engine_url
                            .path_segments_mut()
                            .expect("Invalid settlement engine URL")
                            .push("receiveMessage");
                        let ilp_address_clone = ilp_address.clone();
                        return Box::new(self.http_client.post(settlement_engine_url)
                        .json(&message)
                        .send()
                        .map_err(move |error| {
                            error!("Error sending message to settlement engine: {:?}", error);
                            RejectBuilder {
                                code: ErrorCode::T00_INTERNAL_ERROR,
                                message: b"Error sending message to settlement engine",
                                data: &[],
                                triggered_by: ilp_address_clone.as_ref(),
                            }.build()
                        })
                        .and_then(move |response| {
                            let status = response.status();
                            if status.is_success() {
                                Either::A(response.into_body().concat2().map_err(move |err| {
                                    error!("Error concatenating settlement engine response body: {:?}", err);
                                    RejectBuilder {
                                    code: ErrorCode::T00_INTERNAL_ERROR,
                                    message: b"Error getting settlement engine response",
                                    data: &[],
                                    triggered_by: ilp_address.as_ref(),
                                }.build()
                                })
                                .and_then(|body| {
                                    Ok(FulfillBuilder {
                                        fulfillment: &PEER_FULFILLMENT,
                                        data: body.as_ref(),
                                    }.build())
                                }))
                            } else {
                                error!("Settlement engine rejected message with HTTP error code: {}", response.status());
                                let code = if status.is_client_error() {
                                    ErrorCode::F00_BAD_REQUEST
                                } else {
                                    ErrorCode::T00_INTERNAL_ERROR
                                };
                                Either::B(err(RejectBuilder {
                                    code,
                                    message: format!("Settlement engine rejected request with error code: {}", response.status()).as_str().as_ref(),
                                    data: &[],
                                    triggered_by: ilp_address.as_ref(),
                                }.build()))
                            }
                        }));
                    }
                    Err(error) => {
                        error!(
                            "Got invalid JSON message from account {}: {:?}",
                            request.from.id(),
                            error
                        );
                        return Box::new(err(RejectBuilder {
                            code: ErrorCode::F00_BAD_REQUEST,
                            message: format!("Unable to parse message as JSON: {:?}", error)
                                .as_str()
                                .as_ref(),
                            data: &[],
                            triggered_by: self.ilp_address.as_ref(),
                        }
                        .build()));
                    }
                    _ => {
                        error!("Got invalid settlement message from account {} that could not be parsed as a JSON object", request.from.id());
                        return Box::new(err(RejectBuilder {
                            code: ErrorCode::F00_BAD_REQUEST,
                            message: b"Unable to parse message as a JSON object",
                            data: &[],
                            triggered_by: self.ilp_address.as_ref(),
                        }
                        .build()));
                    }
                }
            } else {
                error!("Got settlement packet from account {} but there is no settlement engine url configured for it", request.from.id());
                return Box::new(err(RejectBuilder {
                    code: ErrorCode::F02_UNREACHABLE,
                    message: &[],
                    data: &[],
                    triggered_by: self.ilp_address.as_ref(),
                }
                .build()));
            }
        }
        Box::new(self.next.handle_request(request))
    }
}
