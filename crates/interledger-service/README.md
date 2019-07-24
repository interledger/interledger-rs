# interledger-service

This is the core abstraction used across the Interledger.rs implementation.

Inspired by [tower](https://github.com/tower-rs), all of the components
of this implementation are "services"
that take a request type and asynchronously return a result.
Every component uses the same interface so that
services can be reused and combined into different bundles of functionality.

The Interledger service traits use requests that contain
ILP Prepare packets and the related `from`/`to` Accounts
and asynchronously return either an ILP Fullfill or Reject packet.
Implementations of Stores (wrappers around
databases) can attach additional information to the Account records,
which are then passed through the service chain.

## Example Service Bundles

The following examples illustrate how different Services can be
chained together to create different bundles of functionality.

### SPSP Sender

`SPSP Client --> ValidatorService --> RouterService --> HttpOutgoingService`

### Connector

`HttpServerService --> ValidatorService --> RouterService --> BalanceAndExchangeRateService --> ValidatorService --> HttpOutgoingService`

### STREAM Receiver

`HttpServerService --> ValidatorService --> StreamReceiverService`
