#![type_length_limit = "5000000"] // this is needed for cargo test

mod btp;
mod exchange_rates;
mod three_nodes;

// Only run prometheus tests if the monitoring feature is turned on
#[cfg(feature = "monitoring")]
mod prometheus;

mod redis_helpers;
mod test_helpers;
