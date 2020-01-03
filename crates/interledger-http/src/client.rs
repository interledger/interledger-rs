use super::{HttpAccount, HttpStore};
use async_trait::async_trait;
use bytes::BytesMut;
use futures::future::TryFutureExt;
use interledger_packet::{Address, ErrorCode, Packet, RejectBuilder};
use interledger_service::*;
use log::{error, trace};
use reqwest::{
    header::{HeaderMap, HeaderName, HeaderValue},
    Client, ClientBuilder, Response as HttpResponse,
};
use secrecy::{ExposeSecret, SecretString};
use std::{convert::TryFrom, marker::PhantomData, sync::Arc, time::Duration};

#[derive(Clone)]
pub struct HttpClientService<S, O, A> {
    client: Client,
    store: Arc<S>,
    next: O,
    account_type: PhantomData<A>,
}

impl<S, O, A> HttpClientService<S, O, A>
where
    S: AddressStore + HttpStore,
    O: OutgoingService<A> + Clone,
    A: HttpAccount,
{
    pub fn new(store: S, next: O) -> Self {
        let mut headers = HeaderMap::with_capacity(2);
        headers.insert(
            HeaderName::from_static("content-type"),
            HeaderValue::from_static("application/octet-stream"),
        );
        let client = ClientBuilder::new()
            .default_headers(headers)
            .timeout(Duration::from_secs(30))
            .build()
            .unwrap();

        HttpClientService {
            client,
            store: Arc::new(store),
            next,
            account_type: PhantomData,
        }
    }
}

#[async_trait]
impl<S, O, A> OutgoingService<A> for HttpClientService<S, O, A>
where
    S: AddressStore + HttpStore + Clone,
    O: OutgoingService<A> + Clone + Sync + Send,
    A: HttpAccount + Clone + Sync + Send,
{
    /// Send an OutgoingRequest to a peer that implements the ILP-Over-HTTP.
    async fn send_request(&mut self, request: OutgoingRequest<A>) -> IlpResult {
        let ilp_address = self.store.get_ilp_address();
        let ilp_address_clone = ilp_address.clone();
        let self_clone = self.clone();
        if let Some(url) = request.to.get_http_url() {
            trace!(
                "Sending outgoing ILP over HTTP packet to account: {} (URL: {})",
                request.to.id(),
                url.as_str()
            );
            let token = request
                .to
                .get_http_auth_token()
                .unwrap_or_else(|| SecretString::new("".to_owned()));
            let header = format!("Bearer {}", token.expose_secret());
            let body = request.prepare.as_ref().to_owned();
            let resp = self_clone
                .client
                .post(url.as_ref())
                .header("authorization", &header)
                .body(body)
                .send()
                .map_err(move |err| {
                    error!("Error sending HTTP request: {:?}", err);
                    let mut code = ErrorCode::T01_PEER_UNREACHABLE;
                    if let Some(status) = err.status() {
                        if status.is_client_error() {
                            code = ErrorCode::F00_BAD_REQUEST
                        }
                    };

                    let message = format!("Error sending ILP over HTTP request: {}", err);
                    RejectBuilder {
                        code,
                        message: message.as_bytes(),
                        triggered_by: Some(&ilp_address),
                        data: &[],
                    }
                    .build()
                })
                .await?;
            parse_packet_from_response(resp, ilp_address_clone).await
        } else {
            self.next.send_request(request).await
        }
    }
}

async fn parse_packet_from_response(response: HttpResponse, ilp_address: Address) -> IlpResult {
    let ilp_address_clone = ilp_address.clone();
    let response = response.error_for_status().map_err(|err| {
        error!("HTTP error sending ILP over HTTP packet: {:?}", err);
        let code = if let Some(status) = err.status() {
            if status.is_client_error() {
                ErrorCode::F02_UNREACHABLE
            } else {
                // TODO more specific errors for rate limiting, etc?
                ErrorCode::T01_PEER_UNREACHABLE
            }
        } else {
            ErrorCode::T00_INTERNAL_ERROR
        };
        RejectBuilder {
            code,
            message: &[],
            triggered_by: Some(&ilp_address),
            data: &[],
        }
        .build()
    })?;

    let ilp_address_clone = ilp_address.clone();
    let body = response
        .bytes()
        .map_err(|err| {
            error!("Error getting HTTP response body: {:?}", err);
            RejectBuilder {
                code: ErrorCode::T01_PEER_UNREACHABLE,
                message: &[],
                triggered_by: Some(&ilp_address_clone),
                data: &[],
            }
            .build()
        })
        .await?;
    // TODO can we get the body as a BytesMut so we don't need to copy?
    let body = BytesMut::from(body.to_vec());
    match Packet::try_from(body) {
        Ok(Packet::Fulfill(fulfill)) => Ok(fulfill),
        Ok(Packet::Reject(reject)) => Err(reject),
        _ => Err(RejectBuilder {
            code: ErrorCode::T01_PEER_UNREACHABLE,
            message: &[],
            triggered_by: Some(&ilp_address_clone),
            data: &[],
        }
        .build()),
    }
}
