# interledger-btp

This crate provides an implementation of [Bilateral Transfer Protocol](https://interledger.org/rfcs/0023-bilateral-transfer-protocol/)
(BTP), an implementation of the [data link layer](https://en.wikipedia.org/wiki/Data_link_layer)
of the Interledger Protocol stack, roughly analogous to [Ethernet](https://en.wikipedia.org/wiki/Ethernet).

BTP utilizes websockets, which makes it suitable for users who
do not have a public internet server.
Users who do not need such functionality may prefer the alternative,
simpler data link layer protocol provided by [the interledger-http crate](https://github.com/interledger-rs/interledger-rs/tree/master/crates/interledger-http).
