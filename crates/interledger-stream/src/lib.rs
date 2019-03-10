//! # interledger-stream
//!
//! Client and server implementations of the Interledger [STREAM](https://github.com/interledger/rfcs/blob/master/0029-stream/0029-stream.md) transport protocol.
//!
//! STREAM is responsible for splitting larger payments and messages into smaller chunks of money and data, and sending them over ILP.
#[macro_use]
extern crate log;
#[macro_use]
extern crate lazy_static;
#[macro_use]
extern crate failure;

mod client;
mod congestion;
mod crypto;
mod error;
mod packet;
mod server;

pub use client::send_money;
pub use error::Error;
pub use server::{ConnectionGenerator, StreamReceiverService};
