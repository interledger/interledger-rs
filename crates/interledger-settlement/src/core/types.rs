use bytes::Bytes;
use futures::Future;
use http::StatusCode;
use interledger_http::error::{ApiError, ApiErrorType, ProblemType};
use interledger_packet::Address;
use interledger_service::Account;
use lazy_static::lazy_static;
use num_bigint::BigUint;
use serde::{Deserialize, Serialize};
use std::ops::{Div, Mul};
use std::str::FromStr;
use url::Url;
use uuid::Uuid;

// Account without an engine error
pub const NO_ENGINE_CONFIGURED_ERROR_TYPE: ApiErrorType = ApiErrorType {
    r#type: &ProblemType::Default,
    title: "No settlement engine configured",
    status: StatusCode::NOT_FOUND,
};

// Number conversion errors
pub const CONVERSION_ERROR_TYPE: ApiErrorType = ApiErrorType {
    r#type: &ProblemType::Default,
    title: "Conversion error",
    status: StatusCode::INTERNAL_SERVER_ERROR,
};

lazy_static! {
    pub static ref SE_ILP_ADDRESS: Address = Address::from_str("peer.settle").unwrap();
}

#[derive(Debug, Clone, Serialize, Deserialize, Eq, PartialEq)]
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

#[derive(Debug, Clone, Serialize, Deserialize, Eq, PartialEq)]
pub enum ApiResponse {
    Default,
    Data(Bytes),
}

/// Trait consumed by the Settlement Engine HTTP API. Every settlement engine
/// MUST implement this trait, so that it can be then be exposed over the API.
pub trait SettlementEngine {
    fn create_account(
        &self,
        account_id: String,
    ) -> Box<dyn Future<Item = ApiResponse, Error = ApiError> + Send>;

    fn delete_account(
        &self,
        account_id: String,
    ) -> Box<dyn Future<Item = ApiResponse, Error = ApiError> + Send>;

    fn send_money(
        &self,
        account_id: String,
        money: Quantity,
    ) -> Box<dyn Future<Item = ApiResponse, Error = ApiError> + Send>;

    fn receive_message(
        &self,
        account_id: String,
        message: Vec<u8>,
    ) -> Box<dyn Future<Item = ApiResponse, Error = ApiError> + Send>;
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
    type Account: Account;

    fn update_balance_for_incoming_settlement(
        &self,
        account_id: Uuid,
        amount: u64,
        idempotency_key: Option<String>,
    ) -> Box<dyn Future<Item = (), Error = ()> + Send>;

    fn refund_settlement(
        &self,
        account_id: Uuid,
        settle_amount: u64,
    ) -> Box<dyn Future<Item = (), Error = ()> + Send>;
}

pub trait LeftoversStore {
    type AccountId: ToString;
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

    /// Clears any uncredited settlement amount associated with the account
    fn clear_uncredited_settlement_amount(
        &self,
        account_id: Self::AccountId,
    ) -> Box<dyn Future<Item = (), Error = ()> + Send>;

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
