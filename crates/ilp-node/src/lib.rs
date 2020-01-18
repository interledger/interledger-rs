// #![type_length_limit = "25000000"]
#![type_length_limit = "1500000"] // needed to cargo build --bin ilp-node --feature "monitoring"

mod instrumentation;
mod node;

#[cfg(feature = "redis")]
mod redis_store;

pub use node::*;
