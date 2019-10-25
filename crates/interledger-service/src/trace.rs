use crate::*;
use tracing_futures::{Instrument, Instrumented};

// TODO see if we can replace this with the tower tracing later
impl<IO, A> IncomingService<A> for Instrumented<IO>
where
    IO: IncomingService<A> + Clone,
    A: Account,
{
    type Future = Instrumented<IO::Future>;

    fn handle_request(&mut self, request: IncomingRequest<A>) -> Self::Future {
        let span = self.span().clone();
        let _enter = span.enter();
        self.inner_mut().handle_request(request).in_current_span()
    }
}

impl<IO, A> OutgoingService<A> for Instrumented<IO>
where
    IO: OutgoingService<A> + Clone,
    A: Account,
{
    type Future = Instrumented<IO::Future>;

    fn send_request(&mut self, request: OutgoingRequest<A>) -> Self::Future {
        let span = self.span().clone();
        let _enter = span.enter();
        self.inner_mut().send_request(request).in_current_span()
    }
}
