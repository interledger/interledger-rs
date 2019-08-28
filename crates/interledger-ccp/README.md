# interledger-ccp
This crate implements the Connector-to-Connector Protocol (CCP) for exchanging routing
information with peers. The `CcpRouteManager` processes Route Update and Route Control
messages from accounts that we are configured to receive routes from and sends route
updates to accounts that we are configured to send updates to.

This populates the routing table implemented in [the interledger-router crate](https://github.com/interledger-rs/interledger-rs/tree/master/crates/interledger-router).
