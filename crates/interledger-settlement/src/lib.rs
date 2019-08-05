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
use bigint::uint::U256;

pub use api::SettlementApi;
pub use client::SettlementClient;
pub use message_service::SettlementMessageService;

use std::ops::Div;
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
    ) -> Box<dyn Future<Item = IdempotentData, Error = ()> + Send>;

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

impl Convert for U256 {
    fn normalize_scale(&self, details: ConvertDetails) -> U256 {
        let from_scale = details.from;
        let to_scale = details.to;
        if from_scale >= to_scale {
            self.overflowing_mul(U256::from(10u64.pow(u32::from(from_scale - to_scale))))
                .0
        } else {
            self.div(U256::from(10u64.pow(u32::from(to_scale - from_scale))))
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
