/// Common backend utils for the IdempotentStore and LeftoversStore traits
/// Only exported if the `backends_common` feature flag is enabled
#[cfg(feature = "backends_common")]
pub mod backends_common;

/// Web service which exposes settlement related endpoints as described in the [RFC](https://interledger.org/rfcs/0038-settlement-engines/),
/// All endpoints are idempotent.
pub mod engines_api;

mod settlement_client;
pub use settlement_client::SettlementClient;

/// Expose useful utilities for implementing idempotent functionalities
pub mod idempotency;

/// Expose useful traits
pub mod types;

use num_bigint::BigUint;
use num_traits::Zero;
use ring::digest::{digest, SHA256};
use types::{Convert, ConvertDetails};

/// Converts a number from a precision to another while taking precision loss into account
///
/// # Examples
/// ```rust
/// # use num_bigint::BigUint;
/// # use interledger_settlement::core::scale_with_precision_loss;
/// assert_eq!(
///     scale_with_precision_loss(BigUint::from(905u32), 9, 11),
///     (BigUint::from(9u32), BigUint::from(5u32))
/// );
///
/// assert_eq!(
///     scale_with_precision_loss(BigUint::from(8053u32), 9, 12),
///     (BigUint::from(8u32), BigUint::from(53u32))
/// );
///
/// assert_eq!(
///     scale_with_precision_loss(BigUint::from(1u32), 9, 6),
///     (BigUint::from(1000u32), BigUint::from(0u32))
/// );
/// ```
pub fn scale_with_precision_loss(
    amount: BigUint,
    local_scale: u8,
    remote_scale: u8,
) -> (BigUint, BigUint) {
    // It's safe to unwrap here since BigUint's normalize_scale cannot fail.
    let scaled = amount
        .normalize_scale(ConvertDetails {
            from: remote_scale,
            to: local_scale,
        })
        .unwrap();

    if local_scale < remote_scale {
        // If we ended up downscaling, scale the value back up back,
        // and return any precision loss
        // note that `from` and `to` are reversed compared to the previous call
        let upscaled = scaled
            .normalize_scale(ConvertDetails {
                from: local_scale,
                to: remote_scale,
            })
            .unwrap();
        let precision_loss = if upscaled < amount {
            amount - upscaled
        } else {
            Zero::zero()
        };
        (scaled, precision_loss)
    } else {
        // there is no need to do anything further if we upscaled
        (scaled, Zero::zero())
    }
}

/// Returns the 32-bytes SHA256 hash of the provided preimage
pub fn get_hash_of(preimage: &[u8]) -> [u8; 32] {
    let mut hash = [0; 32];
    hash.copy_from_slice(digest(&SHA256, preimage).as_ref());
    hash
}
