use crate::*;
use tracing_futures::{Instrument, Instrumented};

// TODO see if we can replace this with the tower tracing later
impl<IO> IncomingService for Instrumented<IO>
where
    IO: IncomingService + Clone,
{
    type Future = Instrumented<IO::Future>;

    fn handle_request(&mut self, request: IncomingRequest) -> Self::Future {
        let span = self.span().clone();
        let _enter = span.enter();
        self.inner_mut().handle_request(request).in_current_span()
    }
}

impl<IO> OutgoingService for Instrumented<IO>
where
    IO: OutgoingService + Clone,
{
    type Future = Instrumented<IO::Future>;

    fn send_request(&mut self, request: OutgoingRequest) -> Self::Future {
        let span = self.span().clone();
        let _enter = span.enter();
        self.inner_mut().send_request(request).in_current_span()
    }
}
