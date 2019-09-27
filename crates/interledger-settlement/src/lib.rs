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
use num_bigint::BigUint;
use std::ops::{Div, Mul};

pub use api::{scale_with_precision_loss, SettlementApi};
pub use client::SettlementClient;
pub use message_service::SettlementMessageService;

lazy_static! {
    pub static ref SE_ILP_ADDRESS: Address = Address::from_str("peer.settle").unwrap();
}

#[derive(Extract, Debug, Clone, Serialize, Deserialize, Eq, PartialEq)]
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

// TODO: Since we still haven't finalized all the settlement details, we might
// end up deciding to add some more values, e.g. some settlement engine uid or similar.
// All instances of this struct should be replaced with Url instances once/if we
// agree that there is no more info required to refer to an engine.
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

pub trait LeftoversStore {
    type AccountId;
    type AssetType: ToString;

    /// Saves the leftover data
    fn save_uncredited_settlement_amount(
        &self,
        // The account id that for which there was a precision loss
        account_id: Self::AccountId,
        // The amount for which precision loss occurred, along with their scale
        uncredited_settlement_amount: (Self::AssetType, u8),
    ) -> Box<dyn Future<Item = (), Error = ()> + Send>;

    /// Returns the leftover data scaled to `local_scale` from the saved scale.
    /// If any precision loss occurs during the scaling, it should be saved as
    /// the new leftover value.
    fn load_uncredited_settlement_amount(
        &self,
        account_id: Self::AccountId,
        local_scale: u8,
    ) -> Box<dyn Future<Item = Self::AssetType, Error = ()> + Send>;

    // Gets the current amount of leftovers in the store
    fn get_uncredited_settlement_amount(
        &self,
        account_id: Self::AccountId,
    ) -> Box<dyn Future<Item = (Self::AssetType, u8), Error = ()> + Send>;
}

#[derive(Debug)]
pub struct ConvertDetails {
    pub from: u8,
    pub to: u8,
}

/// Traits for u64 and f64 asset code conversions for amounts and rates
pub trait Convert {
    type Item: Sized;

    // Returns the scaled result, or an error if there was an overflow
    fn normalize_scale(&self, details: ConvertDetails) -> Result<Self::Item, ()>;
}

impl Convert for u64 {
    type Item = u64;

    fn normalize_scale(&self, details: ConvertDetails) -> Result<Self::Item, ()> {
        let scale_diff = (details.from as i8 - details.to as i8).abs() as u8;
        let scale = 10u64.pow(scale_diff.into());
        let (res, overflow) = if details.to >= details.from {
            self.overflowing_mul(scale)
        } else {
            self.overflowing_div(scale)
        };
        if overflow {
            Err(())
        } else {
            Ok(res)
        }
    }
}

impl Convert for f64 {
    type Item = f64;
    // Not overflow safe. Would require using a package for Big floating point
    // numbers such as BigDecimal
    fn normalize_scale(&self, details: ConvertDetails) -> Result<Self::Item, ()> {
        let scale_diff = (details.from as i8 - details.to as i8).abs() as u8;
        let scale = 10f64.powi(scale_diff.into());
        let res = if details.to >= details.from {
            self * scale
        } else {
            self / scale
        };
        if res == std::f64::INFINITY {
            Err(())
        } else {
            Ok(res)
        }
    }
}

impl Convert for BigUint {
    type Item = BigUint;

    fn normalize_scale(&self, details: ConvertDetails) -> Result<Self::Item, ()> {
        let scale_diff = (details.from as i8 - details.to as i8).abs() as u8;
        let scale = 10u64.pow(scale_diff.into());
        if details.to >= details.from {
            Ok(self.mul(scale))
        } else {
            Ok(self.div(scale))
        }
    }
}

#[cfg(test)]
mod tests {
    /// Tests for the asset conversion
    use super::*;
    use super::{Convert, ConvertDetails};
    use num_traits::cast::FromPrimitive;

    #[test]
    fn biguint_test() {
        let hundred_gwei = BigUint::from_str("100000000000").unwrap();
        assert_eq!(
            hundred_gwei
                .normalize_scale(ConvertDetails { from: 18, to: 9 })
                .unwrap()
                .to_string(),
            BigUint::from_u64(100u64).unwrap().to_string(),
        );
    }

    #[test]
    fn u64_test() {
        // overflows
        let huge_number = std::u64::MAX / 10;
        assert_eq!(
            huge_number
                .normalize_scale(ConvertDetails { from: 1, to: 18 })
                .unwrap_err(),
            (),
        );
        // 1 unit with scale 1, is 1 unit with scale 1
        assert_eq!(
            1u64.normalize_scale(ConvertDetails { from: 1, to: 1 })
                .unwrap(),
            1
        );
        // there's uncredited_settlement_amount for all number slots which do not increase in
        // increments of 10^abs(to_scale-from_scale)
        assert_eq!(
            1u64.normalize_scale(ConvertDetails { from: 2, to: 1 })
                .unwrap(),
            0
        );
        // 1 unit with scale 1, is 10 units with scale 2
        assert_eq!(
            1u64.normalize_scale(ConvertDetails { from: 1, to: 2 })
                .unwrap(),
            10
        );
        // 1 gwei (scale 9) is 1e9 wei (scale 18)
        assert_eq!(
            1u64.normalize_scale(ConvertDetails { from: 9, to: 18 })
                .unwrap(),
            1_000_000_000
        );
        // 1_000_000_000 wei is 1gwei
        assert_eq!(
            1_000_000_000u64
                .normalize_scale(ConvertDetails { from: 18, to: 9 })
                .unwrap(),
            1,
        );
        // 10 units with base 2 is 100 units with base 3
        assert_eq!(
            10u64
                .normalize_scale(ConvertDetails { from: 2, to: 3 })
                .unwrap(),
            100
        );
        // 299 units with base 3 is 29 units with base 2 (0.9 uncredited_settlement_amount)
        assert_eq!(
            299u64
                .normalize_scale(ConvertDetails { from: 3, to: 2 })
                .unwrap(),
            29
        );
        assert_eq!(
            999u64
                .normalize_scale(ConvertDetails { from: 9, to: 6 })
                .unwrap(),
            0
        );
        assert_eq!(
            1000u64
                .normalize_scale(ConvertDetails { from: 9, to: 6 })
                .unwrap(),
            1
        );
        assert_eq!(
            1999u64
                .normalize_scale(ConvertDetails { from: 9, to: 6 })
                .unwrap(),
            1
        );
    }

    #[allow(clippy::float_cmp)]
    #[test]
    fn f64_test() {
        // overflow
        assert_eq!(
            std::f64::MAX
                .normalize_scale(ConvertDetails {
                    from: 1,
                    to: std::u8::MAX,
                })
                .unwrap_err(),
            ()
        );

        // 1 unit with base 1, is 1 unit with base 1
        assert_eq!(
            1f64.normalize_scale(ConvertDetails { from: 1, to: 1 })
                .unwrap(),
            1.0
        );
        // 1 unit with base 10, is 10 units with base 1
        assert_eq!(
            1f64.normalize_scale(ConvertDetails { from: 1, to: 2 })
                .unwrap(),
            10.0
        );
        // 1 sat is 1e9 wei (multiplied by rate)
        assert_eq!(
            1f64.normalize_scale(ConvertDetails { from: 9, to: 18 })
                .unwrap(),
            1_000_000_000.0
        );

        // 1.0 unit with base 2 is 0.1 unit with base 1
        assert_eq!(
            1f64.normalize_scale(ConvertDetails { from: 2, to: 1 })
                .unwrap(),
            0.1
        );
        assert_eq!(
            10.5f64
                .normalize_scale(ConvertDetails { from: 2, to: 1 })
                .unwrap(),
            1.05
        );
        // 100 units with base 3 is 10 units with base 2
        assert_eq!(
            100f64
                .normalize_scale(ConvertDetails { from: 3, to: 2 })
                .unwrap(),
            10.0
        );
        // 299 units with base 3 is 29.9 with base 2
        assert_eq!(
            299f64
                .normalize_scale(ConvertDetails { from: 3, to: 2 })
                .unwrap(),
            29.9
        );

        assert_eq!(
            999f64
                .normalize_scale(ConvertDetails { from: 9, to: 6 })
                .unwrap(),
            0.999
        );
        assert_eq!(
            1000f64
                .normalize_scale(ConvertDetails { from: 9, to: 6 })
                .unwrap(),
            1.0
        );
        assert_eq!(
            1999f64
                .normalize_scale(ConvertDetails { from: 9, to: 6 })
                .unwrap(),
            1.999
        );
    }
}
