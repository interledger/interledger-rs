//! # Interledger.rs
//!
//! A CLI and library bundle for the Rust implementation of the Interledger Protocol stack.

#[macro_use]
extern crate log;
#[cfg(feature = "cli")]
#[macro_use]
extern crate tower_web;

/// ILP Packet (De)Serialization
pub mod packet {
    pub use interledger_packet::*;
}

/// The core abstractions used by Interledger.rs: IncomingService and OutgoingService
pub mod service {
    pub use interledger_service::*;
}

#[doc(hidden)]
#[cfg(feature = "cli")]
pub mod cli;

/// Bilateral Transport Protocol (BTP) client and server
#[cfg(feature = "btp")]
pub mod btp {
    pub use interledger_btp::*;
}

/// ILP-Over-HTTP client and server
#[cfg(feature = "http")]
pub mod http {
    pub use interledger_http::*;
}

/// STREAM Protocol sender and receiver
#[cfg(feature = "stream")]
pub mod stream {
    pub use interledger_stream::*;
}

/// In-memory data store
#[cfg(feature = "store-memory")]
pub mod store_memory {
    pub use interledger_store_memory::*;
}

/// Interledger Dynamic Configuration Protocol (ILDCP)
#[cfg(feature = "ildcp")]
pub mod ildcp {
    pub use interledger_ildcp::*;
}

/// Router that determines the outgoing Account for a request based on the routing table
#[cfg(feature = "router")]
pub mod router {
    pub use interledger_router::*;
}

/// Simple Payment Setup Protocol (SPSP) sender and query responder
#[cfg(feature = "spsp")]
pub mod spsp {
    pub use interledger_spsp::*;
}

/// Miscellaneous services
#[cfg(feature = "service-util")]
pub mod service_util {
    pub use interledger_service_util::*;
}
