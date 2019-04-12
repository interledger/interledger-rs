use bytes::Bytes;
use futures::{
    future::{err, Either},
    Future,
};
use interledger_packet::{ErrorCode, RejectBuilder};
use interledger_service::{Account, BoxedIlpFuture, IncomingRequest, IncomingService};
use std::marker::PhantomData;

pub trait RateLimitAccount: Account {
    fn packets_per_minute_limit(&self) -> Option<u32> {
        None
    }

    fn amount_per_minute_limit(&self) -> Option<u64> {
        None
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RateLimitError {
    PacketLimitExceeded,
    ThroughputLimitExceeded,
    StoreError,
}

pub trait RateLimitStore {
    type Account: RateLimitAccount;

    fn apply_rate_limits(
        &self,
        account: Self::Account,
        prepare_amount: u64,
    ) -> Box<Future<Item = (), Error = RateLimitError> + Send>;
    fn refund_throughput_limit(
        &self,
        account: Self::Account,
        prepare_amount: u64,
    ) -> Box<Future<Item = (), Error = ()> + Send>;
}

#[derive(Clone)]
pub struct RateLimitService<S, T, A> {
    ilp_address: Bytes,
    next: S,
    store: T,
    account_type: PhantomData<A>,
}

impl<S, T, A> RateLimitService<S, T, A>
where
    S: IncomingService<A> + Clone + Send + Sync + 'static,
    T: RateLimitStore<Account = A> + Clone + Send + Sync + 'static,
    A: RateLimitAccount + Sync + 'static,
{
    pub fn new(ilp_address: Bytes, store: T, next: S) -> Self {
        RateLimitService {
            ilp_address,
            next,
            store,
            account_type: PhantomData,
        }
    }
}

impl<S, T, A> IncomingService<A> for RateLimitService<S, T, A>
where
    S: IncomingService<A> + Clone + Send + Sync + 'static,
    T: RateLimitStore<Account = A> + Clone + Send + Sync + 'static,
    A: RateLimitAccount + Sync + 'static,
{
    type Future = BoxedIlpFuture;

    fn handle_request(&mut self, request: IncomingRequest<A>) -> Self::Future {
        let ilp_address = self.ilp_address.clone();
        let mut next = self.next.clone();
        let store = self.store.clone();
        let account = request.from.clone();
        let account_clone = account.clone();
        let prepare_amount = request.prepare.amount();
        let has_throughput_limit = account.amount_per_minute_limit().is_some();
        Box::new(self.store.apply_rate_limits(request.from.clone(), request.prepare.amount())
            .map_err(move |err| {
                let code = match err {
                    RateLimitError::PacketLimitExceeded => {
                        if let Some(limit) = account.packets_per_minute_limit() {
                            warn!("Account {} was rate limited for sending too many packets. Limit is: {} per minute", account.id(), limit);
                        }
                        ErrorCode::T05_RATE_LIMITED
                    },
                    RateLimitError::ThroughputLimitExceeded => {
                        if let Some(limit) = account.amount_per_minute_limit() {
                            warn!("Account {} was throughput limited for trying to send too much money. Limit is: {} per minute", account.id(), limit);
                        }
                        ErrorCode::T04_INSUFFICIENT_LIQUIDITY
                    },
                    RateLimitError::StoreError => ErrorCode::T00_INTERNAL_ERROR,
                };
                RejectBuilder {
                    code,
                    triggered_by: &ilp_address,
                    message: &[],
                    data: &[],
                }.build()
            })
            .and_then(move |_| next.handle_request(request))
            .or_else(move |reject| {
                if has_throughput_limit {
                    Either::A(store.refund_throughput_limit(account_clone, prepare_amount)
                        .then(|result| {
                            if result.is_err() {
                                error!("Error refunding throughput limit: {:?}", result.unwrap_err());
                            }
                            Err(reject)
                        }))
                } else {
                    Either::B(err(reject))
                }
            }))
    }
}
