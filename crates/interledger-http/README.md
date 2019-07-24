# interledger-http

This crate provides an implementation of [ILP over HTTP](https://interledger.org/rfcs/0035-ilp-over-http/),
an implementation of the [data link layer](https://en.wikipedia.org/wiki/Data_link_layer)
of the Interledger Protocol stack, roughly analogous to [Ethernet](https://en.wikipedia.org/wiki/Ethernet).

This is an alternative to the protocol implemented by the
interledger-btp crate, whose main distinguishing feature
is the use of HTTP rather than websockets.
This protocol is intended primarily for server-to-server
communication between peers on the Interledger network.
