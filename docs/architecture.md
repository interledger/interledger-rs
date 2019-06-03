# Interledger.rs Architecture

This document explains some of the main design decisions behind Interledger.rs and how the components fit together.

Note that this document assumes some familiarity with the Interledger Protocol (ILP). If you want to read more about the protocol or familiarize yourself with the packet format and flow, please see the [Interledger Architecture](https://interledger.org/rfcs/0001-interledger-architecture) specification.

**Key Design Features:**
- Every ILP packet is processed by a chain of stateless [`Services`](#services-core-internal-abstraction)
- All state is kept in an underlying database or [`Store`](#stores-database-abstraction)
- All details related to an account or peer are bundled in an [`Account`](#accounts) object, which is loaded from the `Store` and passed through the `Services`
- Nothing is instantiated for each packet or for each account; services that behave differently depending on account-specific details or configuration use methods on the `Account` object to get those details and behave accordingly
- Multiple identical nodes / connectors can be run and pointed at the same underlying database to horizontally scale a deployment for increased throughput
- Settlement is handled by [Settlement Engines](#settlement-engines) that run in separate processes but read from and write to the same database as the Interledger.rs components

## Services - Core Internal Abstraction

### What are Services?

Interledger.rs is designed around a pattern called services, which is an abstraction used to define clients and servers for request / response protocols. Each service accepts a request and asynchronously returns a successful response or error. Services can be chained together to bundle different types of functionality, add handlers for specific requests, or modify requests before passing them to the next service in the chain. This design is inspired by [Tower](https://github.com/tower-rs/tower).

Most components in Interledger.rs define either [`IncomingService`](https://docs.rs/interledger/0/interledger/service/trait.IncomingService.html)s or [`OutgoingService`](https://docs.rs/interledger/0/interledger/service/trait.OutgoingService.html). An `IncomingService` accepts an ILP Prepare packet and a "from" [`Account`](#accounts) (representing which account or peer the packet came from) and returns a [Future](https://docs.rs/futures/0.1.25/trait.Future.html) that resolves to either an ILP Fulfill packet or an ILP Reject packet. An `OutgoingService` is very similar but the request also contains a "to" [`Account`](#accounts) that specifies which account or peer the Prepare packet should be sent to.

Most services implement either the `IncomingService` or `OutgoingService` traits and accept a "next" service that implements that same trait. The service will execute its functionality and then may call the next service to pass the request through the chain. A service that handles a given request, such as the [`IldcpService`](../crates/interledger-ildcp/src/server.rs) may return a Fulfill or Reject without calling the next service.

The [`Router`](https://docs.rs/interledger/0/interledger/router/struct.Router.html) service is unique because it implements the `IncomingService` trait but accepts an `OutgoingService` as the next service. It uses the routing table returned by the [Store](#stores-database-abstraction) and the destination ILP Address from the Prepare packet to determine the "to" `Account` the request should be forwarded to. The "to" account may be another intermediary node or the final recipient.

### Zero-Copy ILP Packet Forwarding

Interledger.rs services operate on deserialized ILP Prepare, Fulfill, and Reject packets. However, this implementation uses zero-copy parsing and the "deserialized" ILP packet object contains the serialized packet buffer and simple pointers to the location of each field of the packet. Querying the fields of the packet gives immutable references to the underlying buffer. Additionally, this library enables the two mutable fields in an ILP Prepare packet (`amount` and `expires_at`) to be changed in-place so that even a forwarding node / connector does not need to copy the packet.

This approach is more convenient than simply passing a serialized buffer between components (because each one needs access to the deserialized data) and more efficient than copying the fields of the packet into an object only to reserialize them later.

### Accounts

Interledger.rs combines details and configuration related to the customers, peers, and upstream providers of a node into [`Account`](https://docs.rs/interledger/0/interledger/service/trait.Account.html) records.

Each service can define traits that the `Account` type passed to it in the `IncomingRequest`s or `OutgoingRequest`s it handles must implement. For example, the [`HttpClientService`](https://docs.rs/interledger/0/interledger/http/struct.HttpClientService.html) requires `Accounts` to provide `get_http_url` and `get_http_auth_header` methods so that the HTTP client knows where to send the outgoing request. A specific `Account` type can implement any or all of these service-specific traits.

`Account` details are generally loaded from the [`Store`](#stores-database-abstraction) and passed through the chain of services along with the Prepare packet as part of the request. This gives each service access to the static account-related properties and configuration it needs to apply its functionality without hitting the underlying database for each one. Another nice benefit of this design is that Rust compiler will not allow a `Store` to be passed to a particular service unless the `Store`'s associated `Account` type implements the required methods.

### Monolith or Microservices

The service pattern enables all of the components to be used separately as microservices _and_ bundled together into an all-in-one node. To make a standalone microservice, a given service, such as the [`StreamReceiverService`](https://docs.rs/interledger/0/interledger/stream/struct.StreamReceiverService.html), can be attached to an [`HttpServerService`](https://docs.rs/interledger/0/interledger/http/struct.HttpServerService.html) so that it will respond to individual Prepare packets forwarded to it via HTTP. Alternatively, the same `StreamReceiverService` can be chained together with other services so that it will handle Prepare packets meant for it and pass along any packets that are meant to be forwarded.

Some bundles of specific functions are available through the [CLI](../interledger/src/cli.rs). If there are other bundles that you would find useful, please feel free to submit a Pull Request to add them!

## Stores - Database Abstraction

The `Store` is the abstraction used for different databases, which can range from in-memory, to [Redis](https://redis.io), to SQL, NoSQL, or others.

Each service that needs access to the database can define traits that the `Store` type it is passed must implement. For example, the `HttpServerService` requires the `Store` to implement a [`get_account_from_http_auth`](https://docs.rs/interledger/0/interledger/http/trait.HttpStore.html) method that it will use to look up the `Account` details for an `IncomingRequest` based on the authentication details used for the incoming HTTP request. The `Router` requires the `Store` to provide [`routing_table`](https://docs.rs/interledger/0/interledger/router/trait.RouterStore.html) and [`get_accounts`](https://docs.rs/interledger/0/interledger/service/trait.AccountStore.html) methods that it uses to determine the next `Account` to send the request to and load the `Account` details from the database.

`Store` types can implement any or all of the service-specific `Store` traits. The Rust compiler ensures that a `Store` passed to a given service implements the trait(s) and methods that that service requires.

This approach to abstracting over different databases allows Interledger.rs to be used on top of multiple types of databases while leveraging the specific capabilities of each one. Instead of having one database abstraction that is the least common denominator of what each service requires, such as a simple key-value store, each service defines exactly what it needs and it is up to the `Store` implementation to provide it. For example, a `Store` implementation on top of a database that provides atomic transactions can use those for balance updates without the balance service needing to implement its own locking mechanism on top.

A further benefit of this approach is that each `Store` implementation can decide how to save and format the data in the underlying database and those choices are completely abstracted away from the services. The `Store` can organize all account-related details into sensible tables and indexes and multiple services can access the same underlying data using the service-specific methods from the traits the `Store` implements.

## Settlement Engines

### Separate Clearing and Settlement

Interledger.rs separates the "clearing" of ILP packets from the "settlement" of balances between directly connected nodes.

The logic for forwarding and processing ILP packets and updating account balances for them is implemented in Rust. External [settlement engines](./settlement-engines) are provided for specific ledgers and "Layer 2" technologies such as payment channels. The settlement engines can be written in any programming language -- ideally whichever has the best SDK for the underlying settlement mechanism.

Settlement engines are run as separate processes from the core Interledger node / connector. This means that even slower settlement engine implementations should not affect the latency or throughput of the Interledger.rs node.

### Common Database

The core Interledger.rs components and external settlement engines talk directly to the same databases. For example, the settlement engines will use the same balances tables as the Interledger node so that they can read the accrued account balance and update it to reflect incoming and outgoing settlements. Settlement engines can also define additional tables or indices, for example to map ledger addresses to `Account` records or to store the latest payment channel claims.

Note that this design means that each settlement engine must separately implement its interactions with different databases. Settlement engines _should_ support all databases for which there are Interledger.rs `Store` implementations.

### Polling Over PubSub

It is recommended for performance reasons that settlement engines poll the underlying database on some configured interval, rather than using a publish / subscribe method to receive real-time balance updates. The Interledger.rs node / connector may process orders of magnitude more ILP packets than the settlement engine can handle. The settlement engine can poll the database at any frequency that makes sense for that settlement method (every couple seconds or minutes for on-ledger transfer-based methods or more often for payment channel-based methods).

### Bilateral Messaging

If a settlement engine requires bilateral messaging, for example to exchange payment channel claims or updates, it is recommended to have a component written in Rust and a separate settlement engine. The Rust service will be added to service chain to handle incoming messages and write relevant details to the underlying database (through a `Store` abstraction). The separate settlement engine will connect to the underlying ledger or blockchain, listen for notifications, and submit transactions such as payment channel closes when applicable.

This design is preferable to having the Interledger.rs node / connector forward ILP Prepare packets with a specific ILP Address prefix to the external settlement engine. Forwarding all packets with a certain prefix to the settlement engine would make it easy to DoS by overwhelming the (not necessarily high-throughput) settlement engine with too many packets.
