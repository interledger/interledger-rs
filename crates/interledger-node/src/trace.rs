use futures::Future;
use interledger::{
    ccp::{CcpRoutingAccount, RoutingRelation},
    packet::{ErrorCode, Fulfill, Reject},
    service::{Account, IncomingRequest, IncomingService, OutgoingRequest, OutgoingService},
};
use std::str;
use tracing::{debug_span, error_span, info, info_span};
use tracing_futures::Instrument;
use uuid::Uuid;

/// Add tracing context for the incoming request.
/// This adds minimal information for the ERROR log
/// level and more information for the DEBUG level.
pub fn trace_incoming<A: Account>(
    request: IncomingRequest<A>,
    mut next: impl IncomingService<A>,
) -> impl Future<Item = Fulfill, Error = Reject> {
    let request_span = error_span!(target: "interledger-node",
        "incoming",
        request.id = %Uuid::new_v4(),
        prepare.destination = %request.prepare.destination(),
        prepare.amount = request.prepare.amount(),
        from.id = %request.from.id()
    );
    let _request_scope = request_span.enter();
    // These details can be looked up by the account ID
    // so don't bother printing them unless we're debugging
    let details_span = debug_span!(target: "interledger-node",
        // This isn't named because its only purpose is to add
        // more details to the request_span context
        "",
        from.username = %request.from.username(),
        from.ilp_address = %request.from.ilp_address(),
        from.asset_code = %request.from.asset_code(),
        from.asset_scale = %request.from.asset_scale(),
    );
    let _details_scope = details_span.enter();

    next.handle_request(request)
        .then(trace_response)
        .in_current_span()
}

/// Add tracing context when the incoming request is
/// being forwarded and turned into an outgoing request.
/// This adds minimal information for the ERROR log
/// level and more information for the DEBUG level.
pub fn trace_forwarding<A: Account>(
    request: OutgoingRequest<A>,
    mut next: impl OutgoingService<A>,
) -> impl Future<Item = Fulfill, Error = Reject> {
    // Here we only include the outgoing details because this will be
    // inside the "incoming" span that includes the other details
    let request_span = error_span!(target: "interledger-node",
        "forwarding",
        to.id = %request.to.id(),
        prepare.amount = request.prepare.amount(),
    );
    let _request_scope = request_span.enter();
    let details_span = debug_span!(target: "interledger-node",
        "",
        to.username = %request.from.username(),
        to.asset_code = %request.from.asset_code(),
        to.asset_scale = %request.from.asset_scale(),
    );
    let _details_scope = details_span.enter();

    next.send_request(request).in_current_span()
}

/// Add tracing context for the outgoing request (created by this node).
/// This adds minimal information for the ERROR log
/// level and more information for the DEBUG level.
pub fn trace_outgoing<A: Account + CcpRoutingAccount>(
    request: OutgoingRequest<A>,
    mut next: impl OutgoingService<A>,
) -> impl Future<Item = Fulfill, Error = Reject> {
    let request_span = error_span!(target: "interledger-node",
        "outgoing",
        request.id = %Uuid::new_v4(),
        prepare.destination = %request.prepare.destination(),
        from.id = %request.from.id(),
        to.id = %request.to.id(),
    );
    let _request_scope = request_span.enter();
    let details_span = debug_span!(target: "interledger-node",
        "",
        from.username = %request.from.username(),
        from.ilp_address = %request.from.ilp_address(),
        from.asset_code = %request.from.asset_code(),
        from.asset_scale = %request.from.asset_scale(),
        to.username = %request.from.username(),
        to.asset_code = %request.from.asset_code(),
        to.asset_scale = %request.from.asset_scale(),
    );
    let _details_scope = details_span.enter();

    // Don't log anything for failed route updates sent to child accounts
    // because there's a good chance they'll be offline
    let ignore_rejects = request.prepare.destination().scheme() == "peer"
        && request.to.routing_relation() == RoutingRelation::Child;
    next.send_request(request)
        .then(move |result| {
            if let Err(ref err) = result {
                if err.code() == ErrorCode::F02_UNREACHABLE && ignore_rejects {
                    return result;
                }
            }
            trace_response(result)
        })
        .in_current_span()
}

/// Log whether the response was a Fulfill or Reject
fn trace_response(result: Result<Fulfill, Reject>) -> Result<Fulfill, Reject> {
    match result {
        Ok(ref fulfill) => {
            debug_span!(target: "interledger-node", "", fulfillment = %hex::encode(fulfill.fulfillment())).in_scope(
                || {
                    info!(target: "interledger-node", result = "fulfill");
                },
            )
        }
        Err(ref reject) => if let Some(ref address) = reject.triggered_by() {
            info_span!(target: "interledger-node",
                "",
                reject.code = %reject.code(),
                reject.message = %str::from_utf8(reject.message()).unwrap_or_default(),
                reject.triggered_by = %address)
        } else {
            info_span!(target: "interledger-node",
                "",
                reject.code = %reject.code(),
                reject.message = %str::from_utf8(reject.message()).unwrap_or_default(),
                reject.triggered_by = "")
        }
        .in_scope(|| {
            info!(target: "interledger-node", result = "reject");
        }),
    };

    result
}
