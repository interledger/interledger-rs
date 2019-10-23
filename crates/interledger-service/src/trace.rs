use crate::*;
use hex;
use std::str;
use tracing::{info, span, Level, Span};
use tracing_futures::{Instrument, Instrumented};
use uuid::Uuid;

// TODO see if we can replace this with the tower tracing later
impl<IO, A> IncomingService<A> for Instrumented<IO>
where
    IO: IncomingService<A> + Clone,
    A: Account,
{
    type Future = Instrumented<BoxedIlpFuture>;

    fn handle_request(&mut self, request: IncomingRequest<A>) -> Self::Future {
        let _parent_scope = self.span().enter();

        // We only include the request.id field if there isn't one in an outer span.
        // This enables using a specific request ID, for example when propagating
        // the request ID across multiple nodes
        let request_span = if !Span::current().has_field("request.id") {
            span!(Level::ERROR,
                "incoming",
                request.id = %Uuid::new_v4(),
                prepare.destination = %request.prepare.destination(),
                prepare.amount = request.prepare.amount(),
                from.id = %request.from.id()
            )
        } else {
            span!(Level::ERROR,
                "incoming",
                prepare.destination = %request.prepare.destination(),
                prepare.amount = request.prepare.amount(),
                from.id = %request.from.id()
            )
        };
        let _request_scope = request_span.enter();

        // These details can be looked up by the account ID
        // so don't bother printing them unless we're debugging
        let details_span = span!(Level::DEBUG,
            "",
            from.username = %request.from.username(),
            from.ilp_address = %request.from.ilp_address(),
            from.asset_code = %request.from.asset_code(),
            from.asset_scale = %request.from.asset_scale(),
        );
        let _details_scope = details_span.enter();
        (Box::new(
            self.clone()
                .into_inner()
                .handle_request(request)
                .then(trace_response),
        ) as BoxedIlpFuture)
            .in_current_span()
    }
}

impl<IO, A> OutgoingService<A> for Instrumented<IO>
where
    IO: OutgoingService<A> + Clone,
    A: Account,
{
    type Future = Instrumented<BoxedIlpFuture>;

    fn send_request(&mut self, request: OutgoingRequest<A>) -> Self::Future {
        let _parent_scope = self.span().enter();
        // Add the "request" and "from" spans only if this call is not already
        // part of a span that includes those details
        // (That means the request was created by this node and did not originate
        // as an IncomingRequest from another node)
        let (outer_span, inner_span) = if Span::current().has_field("prepare.destination") {
            (
                span!(Level::ERROR,
                    "forwarding",
                    to.id = %request.to.id(),
                ),
                span!(Level::DEBUG,
                    "",
                    to.username = %request.from.username(),
                    to.asset_code = %request.from.asset_code(),
                    to.asset_scale = %request.from.asset_scale(),
                ),
            )
        } else {
            (
                span!(Level::ERROR,
                    "outgoing",
                    request.id = %Uuid::new_v4(),
                    prepare.destination = %request.prepare.destination(),
                    prepare.amount = request.prepare.amount(),
                    from.id = %request.from.id(),
                    to.id = %request.to.id(),
                ),
                span!(Level::DEBUG,
                    "",
                    from.username = %request.from.username(),
                    from.ilp_address = %request.from.ilp_address(),
                    from.asset_code = %request.from.asset_code(),
                    from.asset_scale = %request.from.asset_scale(),
                    to.username = %request.from.username(),
                    to.asset_code = %request.from.asset_code(),
                    to.asset_scale = %request.from.asset_scale(),
                ),
            )
        };
        let _outer_scope = outer_span.enter();
        let _inner_scope = inner_span.enter();

        (Box::new(
            self.clone()
                .into_inner()
                .send_request(request)
                .then(trace_response),
        ) as BoxedIlpFuture)
            .in_current_span()
    }
}

fn trace_response(result: Result<Fulfill, Reject>) -> Result<Fulfill, Reject> {
    match result {
        Ok(ref fulfill) => {
            span!(Level::DEBUG, "", fulfillment = %hex::encode(fulfill.fulfillment())).in_scope(
                || {
                    info!(result = "fulfill");
                },
            )
        }
        Err(ref reject) => if let Some(ref address) = reject.triggered_by() {
            span!(Level::INFO, "",
                reject.code = %reject.code(),
                reject.message = %str::from_utf8(reject.message()).unwrap_or_default(),
                reject.triggered_by = %address)
        } else {
            span!(Level::INFO, "",
                reject.code = %reject.code(),
                reject.message = %str::from_utf8(reject.message()).unwrap_or_default(),
                reject.triggered_by = "")
        }
        .in_scope(|| {
            info!(result = "reject");
        }),
    };

    result
}
