#![type_length_limit = "10000000"]
mod btp;
mod exchange_rates;
mod payments_incoming;
mod three_nodes;
mod time_based_settlement;

// Only run prometheus tests if the monitoring feature is turned on
#[cfg(feature = "monitoring")]
mod prometheus;

mod redis_helpers;
mod test_helpers;
