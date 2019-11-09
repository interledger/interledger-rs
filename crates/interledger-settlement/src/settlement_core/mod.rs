/// Common backend utils for the IdempotentStore and LeftoversStore traits
/// Only exported if the `backends_common` feature flag is enabled
#[cfg(feature = "backends_common")]
pub mod backends_common;
/// The REST API for the settlement engines
pub mod engines_api;
/// Expose useful utilities for implementing idempotent functionalities
pub mod idempotency;
/// Expose useful traits
pub mod types;

use num_bigint::BigUint;
use num_traits::Zero;
use ring::digest::{digest, SHA256};
use types::{Convert, ConvertDetails};

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

// Helper function that returns any idempotent data that corresponds to a
// provided idempotency key. It fails if the hash of the input that
// generated the idempotent data does not match the hash of the provided input.
pub fn get_hash_of(preimage: &[u8]) -> [u8; 32] {
    let mut hash = [0; 32];
    hash.copy_from_slice(digest(&SHA256, preimage).as_ref());
    hash
}
