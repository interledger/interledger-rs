use super::{HttpAccount, HttpStore};
use bytes::BytesMut;
use futures::{future::result, Future, Stream};
use interledger_packet::{ErrorCode, Fulfill, Packet, Reject, RejectBuilder};
use interledger_service::*;
use reqwest::{
    header::{HeaderMap, HeaderName, HeaderValue},
    r#async::{Chunk, Client, ClientBuilder, Response as HttpResponse},
};
use std::{marker::PhantomData, sync::Arc, time::Duration};

#[derive(Clone)]
pub struct HttpClientService<S, T, A> {
    client: Client,
    store: Arc<T>,
    next: S,
    account_type: PhantomData<A>,
}

impl<S, T, A> HttpClientService<S, T, A>
where
    T: HttpStore,
    S: OutgoingService<A> + Clone,
    A: HttpAccount,
{
    pub fn new(store: T, next: S) -> Self {
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

impl<S, T, A> OutgoingService<A> for HttpClientService<S, T, A>
where
    S: OutgoingService<A>,
    T: HttpStore,
    A: HttpAccount,
{
    type Future = BoxedIlpFuture;

    /// Send an OutgoingRequest to a peer that implements the ILP-Over-HTTP.
    fn send_request(&mut self, request: OutgoingRequest<A>) -> Self::Future {
        if let Some(url) = request.to.get_http_url() {
            trace!(
                "Sending outgoing ILP over HTTP packet to account: {} (URL: {})",
                request.to.id(),
                url.as_str()
            );
            Box::new(
                self.client
                    .post(url.clone())
                    .header(
                        "authorization",
                        format!("Bearer {}", request.to.get_http_auth_token().unwrap_or("")),
                    )
                    .body(BytesMut::from(request.prepare).freeze())
                    .send()
                    .map_err(|err| {
                        error!("Error sending HTTP request: {:?}", err);
                        RejectBuilder {
                            code: ErrorCode::T01_PEER_UNREACHABLE,
                            message: &[],
                            triggered_by: &[],
                            data: &[],
                        }
                        .build()
                    })
                    .and_then(parse_packet_from_response),
            )
        } else {
            Box::new(self.next.send_request(request))
        }
    }
}

fn parse_packet_from_response(
    response: HttpResponse,
) -> impl Future<Item = Fulfill, Error = Reject> {
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
            triggered_by: &[],
            data: &[],
        }
        .build()
    }))
    .and_then(|response: HttpResponse| {
        let decoder = response.into_body();
        decoder.concat2().map_err(|err| {
            error!("Error getting HTTP response body: {:?}", err);
            RejectBuilder {
                code: ErrorCode::T01_PEER_UNREACHABLE,
                message: &[],
                triggered_by: &[],
                data: &[],
            }
            .build()
        })
    })
    .and_then(|body: Chunk| {
        // TODO can we get the body as a BytesMut so we don't need to copy?
        let body = BytesMut::from(body.to_vec());
        match Packet::try_from(body) {
            Ok(Packet::Fulfill(fulfill)) => Ok(fulfill),
            Ok(Packet::Reject(reject)) => Err(reject),
            _ => Err(RejectBuilder {
                code: ErrorCode::T01_PEER_UNREACHABLE,
                message: &[],
                triggered_by: &[],
                data: &[],
            }
            .build()),
        }
    })
}
