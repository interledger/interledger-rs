#![recursion_limit = "128"]

#[macro_use]
extern crate tower_web;

use bytes::Bytes;
use futures::Future;
use hyper::StatusCode;
use interledger_packet::Address;
use interledger_service::Account;
use lazy_static::lazy_static;
use std::str::FromStr;
use url::Url;

mod api;
mod client;
#[cfg(test)]
mod fixtures;
mod message_service;
#[cfg(test)]
mod test_helpers;
use log::debug;
use num_bigint::BigUint;
use num_traits::cast::ToPrimitive;
use std::ops::{Div, Mul};

pub use api::SettlementApi;
pub use client::SettlementClient;
pub use message_service::SettlementMessageService;

lazy_static! {
    pub static ref SE_ILP_ADDRESS: Address = Address::from_str("peer.settle").unwrap();
}

#[derive(Extract, Debug, Clone, Serialize, Deserialize)]
pub struct Quantity {
    pub amount: String,
    pub scale: u8,
}

impl Quantity {
    pub fn new(amount: impl ToString, scale: u8) -> Self {
        Quantity {
            amount: amount.to_string(),
            scale,
        }
    }
}

pub struct SettlementEngineDetails {
    /// Base URL of the settlement engine
    pub url: Url,
}

pub trait SettlementAccount: Account {
    fn settlement_engine_details(&self) -> Option<SettlementEngineDetails> {
        None
    }
}

pub trait SettlementStore {
    type Account: SettlementAccount;

    fn update_balance_for_incoming_settlement(
        &self,
        account_id: <Self::Account as Account>::AccountId,
        amount: u64,
        idempotency_key: Option<String>,
    ) -> Box<dyn Future<Item = (), Error = ()> + Send>;

    fn refund_settlement(
        &self,
        account_id: <Self::Account as Account>::AccountId,
        settle_amount: u64,
    ) -> Box<dyn Future<Item = (), Error = ()> + Send>;
}

pub type IdempotentData = (StatusCode, Bytes, [u8; 32]);

pub trait IdempotentStore {
    /// Returns the API response that was saved when the idempotency key was used
    /// Also returns a hash of the input data which resulted in the response
    fn load_idempotent_data(
        &self,
        idempotency_key: String,
    ) -> Box<dyn Future<Item = Option<IdempotentData>, Error = ()> + Send>;

    /// Saves the data that was passed along with the api request for later
    /// The store MUST also save a hash of the input, so that it errors out on requests
    fn save_idempotent_data(
        &self,
        idempotency_key: String,
        input_hash: [u8; 32],
        status_code: StatusCode,
        data: Bytes,
    ) -> Box<dyn Future<Item = (), Error = ()> + Send>;
}

#[derive(Debug)]
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
        let scale_diff = (details.from as i8 - details.to as i8).abs() as u8;
        let scale = 10u64.pow(scale_diff.into());
        let num = BigUint::from(*self);
        let num = if details.to >= details.from {
            num.mul(scale)
        } else {
            num.div(scale)
        };
        if let Some(num_u64) = num.to_u64() {
            num_u64
        } else {
            debug!(
                "Overflow during conversion from {} {:?}. Using u64::MAX",
                num, details
            );
            std::u64::MAX
        }
    }
}

impl Convert for f64 {
    // Not overflow safe. Would require using a package for Big floating point
    // numbers such as BigDecimal
    fn normalize_scale(&self, details: ConvertDetails) -> Self {
        let scale_diff = (details.from as i8 - details.to as i8).abs() as u8;
        let scale = 10f64.powi(scale_diff.into());
        if details.to >= details.from {
            self * scale
        } else {
            self / scale
        }
    }
}

impl Convert for BigUint {
    fn normalize_scale(&self, details: ConvertDetails) -> Self {
        let scale_diff = (details.from as i8 - details.to as i8).abs() as u8;
        let scale = 10u64.pow(scale_diff.into());
        if details.to >= details.from {
            self.mul(scale)
        } else {
            self.div(scale)
        }
    }
}

#[cfg(test)]
mod tests {
    /// Tests for the asset conversion
    use super::{Convert, ConvertDetails};

    #[test]
    fn u64_test() {
        // does not overflow
        let huge_number = std::u64::MAX / 10;
        assert_eq!(
            huge_number.normalize_scale(ConvertDetails { from: 1, to: 18 }),
            std::u64::MAX
        );
        // 1 unit with scale 1, is 1 unit with scale 1
        assert_eq!(1u64.normalize_scale(ConvertDetails { from: 1, to: 1 }), 1);
        // there's leftovers for all number slots which do not increase in
        // increments of 10^abs(to_scale-from_scale)
        assert_eq!(1u64.normalize_scale(ConvertDetails { from: 2, to: 1 }), 0);
        // 1 unit with scale 1, is 10 units with scale 2
        assert_eq!(1u64.normalize_scale(ConvertDetails { from: 1, to: 2 }), 10);
        // 1 gwei (scale 9) is 1e9 wei (scale 18)
        assert_eq!(
            1u64.normalize_scale(ConvertDetails { from: 9, to: 18 }),
            1_000_000_000
        );
        // 1_000_000_000 wei is 1gwei
        assert_eq!(
            1_000_000_000u64.normalize_scale(ConvertDetails { from: 18, to: 9 }),
            1,
        );
        // 10 units with base 2 is 100 units with base 3
        assert_eq!(
            10u64.normalize_scale(ConvertDetails { from: 2, to: 3 }),
            100
        );
        // 299 units with base 3 is 29 units with base 2 (0.9 leftovers)
        assert_eq!(
            299u64.normalize_scale(ConvertDetails { from: 3, to: 2 }),
            29
        );
        assert_eq!(999u64.normalize_scale(ConvertDetails { from: 9, to: 6 }), 0);
        assert_eq!(
            1000u64.normalize_scale(ConvertDetails { from: 9, to: 6 }),
            1
        );
        assert_eq!(
            1999u64.normalize_scale(ConvertDetails { from: 9, to: 6 }),
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
            1f64.normalize_scale(ConvertDetails { from: 1, to: 2 }),
            10.0
        );
        // 1 sat is 1e9 wei (multiplied by rate)
        assert_eq!(
            1f64.normalize_scale(ConvertDetails { from: 9, to: 18 }),
            1_000_000_000.0
        );

        // 1.0 unit with base 2 is 0.1 unit with base 1
        assert_eq!(1f64.normalize_scale(ConvertDetails { from: 2, to: 1 }), 0.1);
        assert_eq!(
            10.5f64.normalize_scale(ConvertDetails { from: 2, to: 1 }),
            1.05
        );
        // 100 units with base 3 is 10 units with base 2
        assert_eq!(
            100f64.normalize_scale(ConvertDetails { from: 3, to: 2 }),
            10.0
        );
        // 299 units with base 3 is 29.9 with base 2
        assert_eq!(
            299f64.normalize_scale(ConvertDetails { from: 3, to: 2 }),
            29.9
        );

        assert_eq!(
            999f64.normalize_scale(ConvertDetails { from: 9, to: 6 }),
            0.999
        );
        assert_eq!(
            1000f64.normalize_scale(ConvertDetails { from: 9, to: 6 }),
            1.0
        );
        assert_eq!(
            1999f64.normalize_scale(ConvertDetails { from: 9, to: 6 }),
            1.999
        );
    }
}
