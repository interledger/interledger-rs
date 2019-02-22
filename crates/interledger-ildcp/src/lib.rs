#[macro_use]
extern crate lazy_static;

mod packet;
mod server;
mod store;
mod client;

pub use store::{AccountDetails, IldcpStore};
pub use server::IldcpService;
pub use client::get_ildcp_info;
