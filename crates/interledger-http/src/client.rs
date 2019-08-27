use super::{HttpAccount, HttpStore};
use bytes::BytesMut;
use futures::{future::result, Future, Stream};
use interledger_packet::{Address, ErrorCode, Fulfill, Packet, Reject, RejectBuilder};
use interledger_service::*;
use log::{error, trace};
use reqwest::{
    header::{HeaderMap, HeaderName, HeaderValue},
    r#async::{Chunk, Client, ClientBuilder, Response as HttpResponse},
};
use std::{convert::TryFrom, marker::PhantomData, sync::Arc, time::Duration};

#[derive(Clone)]
pub struct HttpClientService<S, O, A> {
    ilp_address: Address,
    client: Client,
    store: Arc<S>,
    next: O,
    account_type: PhantomData<A>,
}

impl<S, O, A> HttpClientService<S, O, A>
where
    S: HttpStore,
    O: OutgoingService<A> + Clone,
    A: HttpAccount,
{
    pub fn new(ilp_address: Address, store: S, next: O) -> Self {
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
            ilp_address,
            client,
            store: Arc::new(store),
            next,
            account_type: PhantomData,
        }
    }
}

impl<S, O, A> OutgoingService<A> for HttpClientService<S, O, A>
where
    S: HttpStore,
    O: OutgoingService<A>,
    A: HttpAccount,
{
    type Future = BoxedIlpFuture;

    /// Send an OutgoingRequest to a peer that implements the ILP-Over-HTTP.
    fn send_request(&mut self, request: OutgoingRequest<A>) -> Self::Future {
        let ilp_address = self.ilp_address.clone();
        let ilp_address_clone = ilp_address.clone();
        if let Some(url) = request.to.get_http_url() {
            trace!(
                "Sending outgoing ILP over HTTP packet to account: {} (URL: {})",
                request.to.id(),
                url.as_str()
            );
            Box::new(
                self.client
                    .post(url.as_ref())
                    .header(
                        "authorization",
                        format!("Bearer {}", request.to.get_http_auth_token().unwrap_or("")),
                    )
                    .body(BytesMut::from(request.prepare).freeze())
                    .send()
                    .map_err(move |err| {
                        error!("Error sending HTTP request: {:?}", err);
                        RejectBuilder {
                            code: ErrorCode::T01_PEER_UNREACHABLE,
                            message: &[],
                            triggered_by: Some(&ilp_address),
                            data: &[],
                        }
                        .build()
                    })
                    .and_then(move |resp| parse_packet_from_response(resp, ilp_address_clone)),
            )
        } else {
            Box::new(self.next.send_request(request))
        }
    }
}

fn parse_packet_from_response(
    response: HttpResponse,
    ilp_address: Address,
) -> impl Future<Item = Fulfill, Error = Reject> {
    let ilp_address_clone = ilp_address.clone();
    result(response.error_for_status().map_err(|err| {
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
    }))
    .and_then(move |response: HttpResponse| {
        let ilp_address_clone = ilp_address.clone();
        let decoder = response.into_body();
        decoder.concat2().map_err(move |err| {
            error!("Error getting HTTP response body: {:?}", err);
            RejectBuilder {
                code: ErrorCode::T01_PEER_UNREACHABLE,
                message: &[],
                triggered_by: Some(&ilp_address_clone.clone()),
                data: &[],
            }
            .build()
        })
    })
    .and_then(move |body: Chunk| {
        // TODO can we get the body as a BytesMut so we don't need to copy?
        let body = BytesMut::from(body.to_vec());
        match Packet::try_from(body) {
            Ok(Packet::Fulfill(fulfill)) => Ok(fulfill),
            Ok(Packet::Reject(reject)) => Err(reject),
            _ => Err(RejectBuilder {
                code: ErrorCode::T01_PEER_UNREACHABLE,
                message: &[],
                triggered_by: Some(&ilp_address_clone.clone()),
                data: &[],
            }
            .build()),
        }
    })
}
