//! # interledger-service-util
//!
//! Miscellaneous, small Interledger Services.

#[macro_use]
extern crate log;

mod balance_service;
mod echo_service;
mod exchange_rates_service;
mod expiry_shortener_service;
mod max_packet_amount_service;
mod rate_limit_service;
mod validator_service;

pub use self::balance_service::{BalanceService, BalanceStore};
pub use self::echo_service::EchoService;
pub use self::exchange_rates_service::{ExchangeRateService, ExchangeRateStore};
pub use self::expiry_shortener_service::{
    ExpiryShortenerService, RoundTripTimeAccount, DEFAULT_ROUND_TRIP_TIME,
};
pub use self::max_packet_amount_service::{MaxPacketAmountAccount, MaxPacketAmountService};
pub use self::rate_limit_service::{
    RateLimitAccount, RateLimitError, RateLimitService, RateLimitStore,
};
pub use self::validator_service::ValidatorService;

pub struct ConvertDetails {
    pub from: u8,
    pub to: u8,
}

/// Traits for u64 and f64 asset code conversions for amounts and rates
pub trait Convert {
    fn normalize_scale(&self, details: ConvertDetails) -> Self;
}

impl Convert for u64 {
    fn normalize_scale(&self, details: ConvertDetails) -> Self {
        let from_scale = details.from;
        let to_scale = details.to;
        if from_scale >= to_scale {
            self * 10u64.pow(u32::from(from_scale - to_scale))
        } else {
            self / 10u64.pow(u32::from(to_scale - from_scale))
        }
    }
}

impl Convert for f64 {
    fn normalize_scale(&self, details: ConvertDetails) -> Self {
        let from_scale = details.from;
        let to_scale = details.to;
        if from_scale >= to_scale {
            self * 10f64.powi(i32::from(from_scale - to_scale))
        } else {
            self / 10f64.powi(i32::from(to_scale - from_scale))
        }
    }
}

#[cfg(test)]
mod tests {
    /// Tests for the asset conversion
    use super::{Convert, ConvertDetails};

    #[test]
    fn u64_test() {
        // 1 unit with base 1, is 1 unit with base 1
        assert_eq!(1u64.normalize_scale(ConvertDetails { from: 1, to: 1 }), 1);
        // 1 unit with base 10, is 10 units with base 1
        assert_eq!(1u64.normalize_scale(ConvertDetails { from: 2, to: 1 }), 10);
        // 1 wei is 1e9 sats (multiplied by rate)
        assert_eq!(
            1u64.normalize_scale(ConvertDetails { from: 18, to: 9 }),
            1_000_000_000
        );

        // there's leftovers for all number slots which do not increase in
        // increments of 10^{to_scale-from_scale}
        assert_eq!(1u64.normalize_scale(ConvertDetails { from: 1, to: 2 }), 0);
        assert_eq!(10u64.normalize_scale(ConvertDetails { from: 1, to: 2 }), 1);
        // 100 units with base 2 is 10 units with base 3
        assert_eq!(
            100u64.normalize_scale(ConvertDetails { from: 2, to: 3 }),
            10
        );
        // 299 units with base 2 is 10 units with base 3 plus 99 leftovers
        assert_eq!(
            100u64.normalize_scale(ConvertDetails { from: 2, to: 3 }),
            10
        );

        assert_eq!(999u64.normalize_scale(ConvertDetails { from: 6, to: 9 }), 0);
        assert_eq!(
            1000u64.normalize_scale(ConvertDetails { from: 6, to: 9 }),
            1
        );
        assert_eq!(
            1999u64.normalize_scale(ConvertDetails { from: 6, to: 9 }),
            1
        ); // 5 is leftovers, maybe we should return it?

        // allow making sub-sat micropayments
        assert_eq!(1u64.normalize_scale(ConvertDetails { from: 9, to: 18 }), 0);
        assert_eq!(
            1_000_000_000u64.normalize_scale(ConvertDetails { from: 9, to: 18 }),
            1
        );
    }

    #[allow(clippy::float_cmp)]
    #[test]
    fn f64_test() {
        // 1 unit with base 1, is 1 unit with base 1
        assert_eq!(1f64.normalize_scale(ConvertDetails { from: 1, to: 1 }), 1.0);
        // 1 unit with base 10, is 10 units with base 1
        assert_eq!(
            1f64.normalize_scale(ConvertDetails { from: 2, to: 1 }),
            10.0
        );
        // 1 wei is 1e9 sats (multiplied by rate)
        assert_eq!(
            1f64.normalize_scale(ConvertDetails { from: 18, to: 9 }),
            1_000_000_000.0
        );

        // 1.0 unit with base 1 is 0.1 unit with base 2
        assert_eq!(1f64.normalize_scale(ConvertDetails { from: 1, to: 2 }), 0.1);
        assert_eq!(
            10f64.normalize_scale(ConvertDetails { from: 1, to: 2 }),
            1.0
        );
        // 100 units with base 2 is 10 units with base 3
        assert_eq!(
            100f64.normalize_scale(ConvertDetails { from: 2, to: 3 }),
            10.0
        );
        // 299 units with base 2 is 10 units with base 3 plus 99 leftovers
        assert_eq!(
            100f64.normalize_scale(ConvertDetails { from: 2, to: 3 }),
            10.0
        );

        assert_eq!(
            999f64.normalize_scale(ConvertDetails { from: 6, to: 9 }),
            0.999
        );
        assert_eq!(
            1000f64.normalize_scale(ConvertDetails { from: 6, to: 9 }),
            1.0
        );
        assert_eq!(
            1999f64.normalize_scale(ConvertDetails { from: 6, to: 9 }),
            1.999
        );
    }
}
