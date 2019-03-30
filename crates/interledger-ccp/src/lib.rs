#[macro_use]
extern crate log;
#[macro_use]
extern crate lazy_static;

use bytes::Bytes;

#[cfg(test)]
mod fixtures;
mod packet;
mod routing_table;
mod server;

pub use server::CcpServerService;

/// DefineCcpAccountethods Account types need to be used by the CCP Service
pub trait CcpAccount {
    /// Indicates whether we should send CCP Route Updates to this account
    fn should_send_routes(&self) -> bool {
        false
    }

    /// Indicates whether we should accept CCP Route Update Requests from this account
    fn should_receive_routes(&self) -> bool {
        false
    }

    /// Address prefixes that this account has been configured to handle
    fn configured_routes(&self) -> Vec<Bytes> {
        Vec::new()
    }
}
}
