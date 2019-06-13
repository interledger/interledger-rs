use interledger_service::{Account, OutgoingRequest, OutgoingService};
use std::time::Duration;

pub const DEFAULT_ROUND_TRIP_TIME: u64 = 500; // milliseconds?

pub trait RoundTripTimeAccount: Account {
    /// Estimate of how long we expect it to take to send a message to this
    fn round_trip_time(&self) -> u64 {
        DEFAULT_ROUND_TRIP_TIME
    }
}

#[derive(Clone)]
pub struct ExpiryShortenerService<O> {
    next: O,
}

impl<O> ExpiryShortenerService<O> {
    pub fn new(next: O) -> Self {
        ExpiryShortenerService { next }
    }
}

impl<O, A> OutgoingService<A> for ExpiryShortenerService<O>
where
    O: OutgoingService<A>,
    A: RoundTripTimeAccount,
{
    type Future = O::Future;

    fn send_request(&mut self, mut request: OutgoingRequest<A>) -> Self::Future {
        let time_to_subtract = request.from.round_trip_time() + request.to.round_trip_time();
        let new_expiry = request.prepare.expires_at() - Duration::from_millis(time_to_subtract);
        request.prepare.set_expires_at(new_expiry);
        self.next.send_request(request)
    }
}