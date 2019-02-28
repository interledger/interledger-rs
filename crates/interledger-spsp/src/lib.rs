#[macro_use]
extern crate log;
#[macro_use]
extern crate serde_derive;
#[macro_use]
extern crate failure;

mod client;

pub use client::{pay, query, SpspResponse};
