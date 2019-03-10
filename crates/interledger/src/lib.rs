//! # Interledger.rs
//!
//! A CLI for using the Rust implementation of the Interledger Protocol stack.
//!
//! See the component crates for details on how the implementation works:
//!   - [interledger-btp](https://docs.rs/interledger-btp) - Bilateral Transfer Protocol (BTP)
//!   - [interledger-http](https://docs.rs/interledger-http) - ILP-Over-HTTP
//!   - [interledger-ildcp](https://docs.rs/interledger-ildcp) - Interledger Dynamic Configuration Protocol (ILDCP)
//!   - [interledger-packet](https://docs.rs/interledger-packet) - (De)Serialization for ILP Packets
//!   - [interledger-router](https://docs.rs/interledger-router) - Simple router for Interledger Requests
//!   - [interledger-service](https://docs.rs/interledger-service) - Core abstraction used throughout this implementation
//!   - [interledger-service-util](https://docs.rs/interledger-service-util) - Miscellaneous services
//!   - [interledger-spsp](https://docs.rs/interledger-spsp) - Simple Payment Setup Protocol (SPSP)
//!   - [interledger-store-memory](https://docs.rs/interledger-store-memory) - In-memory Store for stateless services and testing
//!   - [interledger-store-redis](https://docs.rs/interledger-store-redis) - Redis-backed store for single- and multi-connector clusters
//!   - [interledger-stream](https://docs.rs/interledger-stream) - STREAM transport protocol
//!   - [interledger-test-helpers](https://docs.rs/interledger-test-helpers) - Helper functions and structs for testing

mod moneyd;
mod spsp;

pub use moneyd::*;
pub use spsp::*;
