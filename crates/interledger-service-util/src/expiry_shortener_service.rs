use interledger_service::{Account, OutgoingRequest, OutgoingService};
use std::time::Duration;

pub const DEFAULT_ROUND_TRIP_TIME: u64 = 500;

pub trait RoundTripTimeAccount: Account {
    fn round_trip_time(&self) -> u64 {
        DEFAULT_ROUND_TRIP_TIME
    }
}

/// # Expiry Shortener Service
///
/// Each node shortens the `Prepare` packet's expiry duration before passing it on.
/// Nodes shorten the expiry duration so that even if the packet is fulfilled just before the expiry,
/// they will still have enough time to pass the fulfillment to the previous node before it expires.
///
/// This service reduces the expiry time of each packet before forwarding it out.
/// Requires a `RoundtripTimeAccount` and _no store_
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

    /// On send request:
    /// 1. Get the sender and receiver's roundtrip time (default 1000ms)
    /// 2. Reduce the packet's expiry by that amount
    /// 3. Forward the request
    fn send_request(&mut self, mut request: OutgoingRequest<A>) -> Self::Future {
        let time_to_subtract = request.from.round_trip_time() + request.to.round_trip_time();
        let new_expiry = request.prepare.expires_at() - Duration::from_millis(time_to_subtract);
        request.prepare.set_expires_at(new_expiry);
        self.next.send_request(request)
    }
}
