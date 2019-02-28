use bytes::Bytes;
use futures::future::err;
use interledger_packet::{ErrorCode, RejectBuilder};
use interledger_service::*;

#[derive(Clone)]
pub struct RejecterService {
    ilp_address: Bytes,
}

impl RejecterService {
    pub fn new(ilp_address: &[u8]) -> Self {
        RejecterService {
            ilp_address: Bytes::from(ilp_address),
        }
    }

    pub fn default() -> Self {
        RejecterService {
            ilp_address: Bytes::new(),
        }
    }
}

impl IncomingService for RejecterService {
    type Future = BoxedIlpFuture;

    fn handle_request(&mut self, request: IncomingRequest) -> Self::Future {
        debug!("Automatically rejecting request: {:?}", request);
        Box::new(err(RejectBuilder {
            code: ErrorCode::F02_UNREACHABLE,
            message: &[],
            triggered_by: &self.ilp_address[..],
            data: &[],
        }
        .build()))
    }
}

impl OutgoingService for RejecterService {
    type Future = BoxedIlpFuture;

    fn send_request(&mut self, request: OutgoingRequest) -> Self::Future {
        debug!("Automatically rejecting request: {:?}", request);
        Box::new(err(RejectBuilder {
            code: ErrorCode::F02_UNREACHABLE,
            message: &[],
            triggered_by: &self.ilp_address[..],
            data: &[],
        }
        .build()))
    }
}
