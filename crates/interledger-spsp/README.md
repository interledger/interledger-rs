# interledger-spsp

Client and server implementations of the [Simple Payment Setup Protocol (SPSP)](https://interledger.org/rfcs/0009-simple-payment-setup-protocol/),
an application-layer protocol within the Interledger Protocol Suite.

This uses a simple HTTPS request to establish a shared key
between the sender and receiver that is used to
authenticate ILP packets sent between them.
SPSP uses the STREAM transport protocol for sending money and data over ILP.
