// #![type_length_limit = "25000000"]
// #![type_length_limit = "1500000"] // needed to cargo build --bin ilp-node --feature "monitoring"
#![type_length_limit = "80000000"] // needed to cargo build --all-features --all-targets

mod instrumentation;
mod node;

#[cfg(feature = "redis")]
mod redis_store;

pub use node::*;
