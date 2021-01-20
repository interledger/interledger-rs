use async_trait::async_trait;
use bytes::Bytes;
use http::StatusCode;
use interledger_errors::{ApiError, ApiErrorType, ProblemType};
use interledger_errors::{LeftoversStoreError, SettlementStoreError};
use interledger_packet::Address;
use interledger_service::Account;
use num_bigint::BigUint;
use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};
use std::ops::{Div, Mul};
use std::str::FromStr;
use url::Url;
use uuid::Uuid;

/// No Engine Configured for Account error type (404 Not Found)
pub const NO_ENGINE_CONFIGURED_ERROR_TYPE: ApiErrorType = ApiErrorType {
    r#type: &ProblemType::Default,
    title: "No settlement engine configured",
    status: StatusCode::NOT_FOUND,
};

/// Number Conversion error type (404 Not Found)
pub const CONVERSION_ERROR_TYPE: ApiErrorType = ApiErrorType {
    r#type: &ProblemType::Default,
    title: "Conversion error",
    status: StatusCode::INTERNAL_SERVER_ERROR,
};

/// The Settlement ILP Address as defined in the [RFC](https://interledger.org/rfcs/0038-settlement-engines/)
pub static SE_ILP_ADDRESS: Lazy<Address> = Lazy::new(|| Address::from_str("peer.settle").unwrap());

/// The Quantity object as defined in the [RFC](https://interledger.org/rfcs/0038-settlement-engines/)
/// An amount denominated in some unit of a single, fungible asset.
/// (Since each account is denominated in a single asset, the type of asset is implied.)
#[derive(Debug, Clone, Serialize, Deserialize, Eq, PartialEq)]
pub struct Quantity {
    /// Amount of the unit, which is a non-negative integer.
    /// This amount is encoded as a string to ensure no precision
    /// is lost on platforms that don't natively support arbitrary precision integers.
    pub amount: String,
    /// Asset scale of the unit
    pub scale: u8,
}

impl Quantity {
    /// Creates a new Quantity object
    pub fn new(amount: impl ToString, scale: u8) -> Self {
        Quantity {
            amount: amount.to_string(),
            scale,
        }
    }
}

/// Helper enum allowing API responses to not specify any data and let the consumer
/// of the call decide what to do with the success value
// TODO: could this maybe be omitted and replaced with Option<Bytes> in Responses?
#[derive(Debug, Clone, Serialize, Deserialize, Eq, PartialEq)]
pub enum ApiResponse {
    /// The API call succeeded without any returned data.
    Default,
    /// The API call returned some data which should be consumed.
    Data(Bytes),
}

/// Type alias for Result over [`ApiResponse`](./enum.ApiResponse.html),
/// and [`ApiError`](../../../interledger_http/error/struct.ApiError.html)
pub type ApiResult = Result<ApiResponse, ApiError>;

/// Trait consumed by the Settlement Engine HTTP API. Every settlement engine
/// MUST implement this trait, so that it can be then be exposed over the API.
#[async_trait]
pub trait SettlementEngine {
    /// Informs the settlement engine that a new account was created
    /// within the accounting system using the given account identifier.
    /// The settlement engine MAY perform tasks as a prerequisite to settle with the account.
    /// For example, a settlement engine implementation might send messages to
    /// the peer to exchange ledger identifiers or to negotiate settlement-related fees.
    async fn create_account(&self, account_id: String) -> ApiResult;

    /// Instructs the settlement engine that an account was deleted.
    async fn delete_account(&self, account_id: String) -> ApiResult;

    /// Asynchronously send an outgoing settlement. The accounting system sends this request and accounts for outgoing settlements.
    async fn send_money(&self, account_id: String, money: Quantity) -> ApiResult;

    /// Process and respond to an incoming message from the peer's settlement engine.
    /// The connector sends this request when it receives an incoming settlement message
    /// from the peer, and returns the response message back to the peer.
    async fn receive_message(&self, account_id: String, message: Vec<u8>) -> ApiResult;
}

// TODO: Since we still haven't finalized all the settlement details, we might
// end up deciding to add some more values, e.g. some settlement engine uid or similar.
// All instances of this struct should be replaced with Url instances once/if we
// agree that there is no more info required to refer to an engine.
/// The details associated with a settlement engine
pub struct SettlementEngineDetails {
    /// Base URL of the settlement engine
    pub url: Url,
}

/// Extension trait for [Account](../interledger_service/trait.Account.html) with [settlement](https://interledger.org/rfcs/0038-settlement-engines/) related information
pub trait SettlementAccount: Account {
    /// The [SettlementEngineDetails](./struct.SettlementEngineDetails.html) (if any) associated with that account
    fn settlement_engine_details(&self) -> Option<SettlementEngineDetails> {
        None
    }
}

#[async_trait]
/// Trait used by the connector to adjust account balances on settlement events
pub trait SettlementStore {
    type Account: Account;

    /// Increases the account's balance/prepaid amount by the provided amount
    ///
    /// This is optionally idempotent. If the same idempotency_key is provided
    /// then no database operation must happen. If there is an idempotency
    /// conflict (same idempotency key, different inputs to function) then
    /// it should return an error
    async fn update_balance_for_incoming_settlement(
        &self,
        account_id: Uuid,
        amount: u64,
        idempotency_key: Option<String>,
    ) -> Result<(), SettlementStoreError>;

    /// Increases the account's balance by the provided amount.
    /// Only call this if a settlement request has failed
    async fn refund_settlement(
        &self,
        account_id: Uuid,
        settle_amount: u64,
    ) -> Result<(), SettlementStoreError>;
}

/// Trait used by the connector and engine to track amounts which should have been
/// settled but were not due to precision loss
#[async_trait]
pub trait LeftoversStore {
    type AccountId: ToString;
    /// The data type that the store uses for tracking numbers.
    type AssetType: ToString;

    /// Saves the leftover data
    ///
    /// @dev:
    /// If your store needs to support Big Integers but cannot, consider setting AssetType to String,
    /// and then proceed to save a list of uncredited amounts as strings which would get loaded and summed
    /// by the load_uncredited_settlement_amount and get_uncredited_settlement_amount
    /// functions
    async fn save_uncredited_settlement_amount(
        &self,
        // The account id that for which there was a precision loss
        account_id: Self::AccountId,
        // The amount for which precision loss occurred, along with their scale
        uncredited_settlement_amount: (Self::AssetType, u8),
    ) -> Result<(), LeftoversStoreError>;

    /// Returns the leftover data scaled to `local_scale` from the saved scale.
    /// If any precision loss occurs during the scaling, it is be saved as
    /// the new leftover value.
    ///
    /// @dev:
    /// If the store needs to support Big Integers but cannot, consider setting AssetType to String,
    /// save a list of uncredited settlement amounts, and load them and sum them in this function
    async fn load_uncredited_settlement_amount(
        &self,
        account_id: Self::AccountId,
        local_scale: u8,
    ) -> Result<Self::AssetType, LeftoversStoreError>;

    /// Clears any uncredited settlement amount associated with the account
    async fn clear_uncredited_settlement_amount(
        &self,
        account_id: Self::AccountId,
    ) -> Result<(), LeftoversStoreError>;

    /// Gets the current amount of leftovers in the store
    async fn get_uncredited_settlement_amount(
        &self,
        account_id: Self::AccountId,
    ) -> Result<(Self::AssetType, u8), LeftoversStoreError>;
}

/// Helper struct for converting a quantity's amount from one asset scale to another
#[derive(Debug)]
pub struct ConvertDetails {
    pub from: u8,
    pub to: u8,
}

/// Helper trait for u64 and f64 asset code conversions for amounts and rates
pub trait Convert {
    type Item: Sized;

    /// Returns the scaled result, or an error if there was an overflow
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
        assert!(huge_number
            .normalize_scale(ConvertDetails { from: 1, to: 18 })
            .is_err(),);
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
        assert!(std::f64::MAX
            .normalize_scale(ConvertDetails {
                from: 1,
                to: std::u8::MAX,
            })
            .is_err(),);

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
