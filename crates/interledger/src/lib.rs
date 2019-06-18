//! # Interledger.rs
//!
//! A CLI and library bundle for the Rust implementation of the Interledger Protocol stack.
#![recursion_limit = "128"]

#[cfg(feature = "cli")]
#[macro_use]
extern crate log;

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
#[cfg(feature = "cli")]
pub mod node;

/// Bilateral Transport Protocol (BTP) client and server
#[cfg(feature = "btp")]
pub mod btp {
    //! # interledger-btp
    //!
    //! Client and server implementations of the [Bilateral Transport Protocol (BTP)](https://github.com/interledger/rfcs/blob/master/0023-bilateral-transfer-protocol/0023-bilateral-transfer-protocol.md).
    //! This is a WebSocket-based protocol for exchanging ILP packets between directly connected peers.
    //!
    //! Because this protocol uses WebSockets, only one party needs to have a publicly-accessible HTTPS
    //! endpoint but both sides can send and receive ILP packets.
    pub use interledger_btp::*;
}

/// Connector-to-Connector Protocol (CCP) routing manager
#[cfg(feature = "ccp")]
pub mod ccp {
    //! # interledger-ccp
    //!
    //! This crate implements the Connector-to-Connector Protocol (CCP) for exchanging routing
    //! information with peers. The `CcpRouteManager` processes Route Update and Route Control
    //! messages from accounts that we are configured to receive routes from and sends route
    //! updates to accounts that we are configured to send updates to.
    //!
    //! The `CcpRouteManager` writes changes to the routing table to the store so that the
    //! updates are used by the `Router` to forward incoming packets to the best next hop
    //! we know about.
    pub use interledger_ccp::*;
}

/// ILP-Over-HTTP client and server
#[cfg(feature = "http")]
pub mod http {
    //! # interledger-http
    //!
    //! Client and server implementations of the [ILP-Over-HTTP](https://github.com/interledger/rfcs/blob/master/0035-ilp-over-http/0035-ilp-over-http.md) bilateral communication protocol.
    //! This protocol is intended primarily for server-to-server communication between peers on the Interledger network.
    pub use interledger_http::*;
}

/// STREAM Protocol sender and receiver
#[cfg(feature = "stream")]
pub mod stream {
    //! # interledger-stream
    //!
    //! Client and server implementations of the Interledger [STREAM](https://github.com/interledger/rfcs/blob/master/0029-stream/0029-stream.md) transport protocol.
    //!
    //! STREAM is responsible for splitting larger payments and messages into smaller chunks of money and data, and sending them over ILP.
    pub use interledger_stream::*;
}

/// In-memory data store
#[cfg(feature = "store-memory")]
pub mod store_memory {
    //! # interledger-store-memory
    //!
    //! A simple in-memory store intended primarily for testing and
    //! stateless sender/receiver services that are passed all of the
    //! relevant account details when the store is instantiated.
    pub use interledger_store_memory::*;
}

/// Interledger Dynamic Configuration Protocol (ILDCP)
#[cfg(feature = "ildcp")]
pub mod ildcp {
    //! # interledger-ildcp
    //!
    //! Client and server implementations of the [Interledger Dynamic Configuration Protocol (ILDCP)](https://github.com/interledger/rfcs/blob/master/0031-dynamic-configuration-protocol/0031-dynamic-configuration-protocol.md).
    //!
    //! This is used by clients to query for their ILP address and asset details such as asset code and scale.
    pub use interledger_ildcp::*;
}

/// Router that determines the outgoing Account for a request based on the routing table
#[cfg(feature = "router")]
pub mod router {
    //! # interledger-router
    //!
    //! A service that routes ILP Prepare packets to the correct next
    //! account based on the ILP address in the Prepare packet based
    //! on the routing table.
    //!
    //! A routing table could be as simple as a single entry for the empty prefix
    //! ("") that will route all requests to a specific outgoing account.
    //!
    //! Note that the Router is not responsible for building the routing table,
    //! only using the information provided by the store. The routing table in the
    //! store can either be configured or populated using the `CcpRouteManager`
    //! (see the `interledger-ccp` crate for more details).
    pub use interledger_router::*;
}

/// Simple Payment Setup Protocol (SPSP) sender and query responder
#[cfg(feature = "spsp")]
pub mod spsp {
    //! # interledger-spsp
    //!
    //! Client and server implementations of the [Simple Payment Setup Protocol (SPSP)](https://github.com/interledger/rfcs/blob/master/0009-simple-payment-setup-protocol/0009-simple-payment-setup-protocol.md).
    //!
    //! This uses a simple HTTPS request to establish a shared key between the sender and receiver that is used to
    //! authenticate ILP packets sent between them. SPSP uses the STREAM transport protocol for sending money and data over ILP.
    pub use interledger_spsp::*;
}

/// Miscellaneous services
#[cfg(feature = "service-util")]
pub mod service_util {
    pub use interledger_service_util::*;
}
