<p align="center">
  <img src="interledger-rs.svg" width="700" alt="Interledger.rs">
</p>

---
> Interledger implementation in Rust :money_with_wings:

[![crates.io](https://img.shields.io/crates/v/interledger.svg)](https://crates.io/crates/interledger)
[![CircleCI](https://circleci.com/gh/emschwartz/interledger-rs.svg?style=shield)](https://circleci.com/gh/emschwartz/interledger-rs)
[![codecov](https://codecov.io/gh/emschwartz/interledger-rs/branch/master/graph/badge.svg)](https://codecov.io/gh/emschwartz/interledger-rs)
[![Interledger.rs Documentation](https://docs.rs/interledger/badge.svg)](https://docs.rs/interledger)

## Understanding Interledger.rs
- [API Docs](https://docs.rs/interledger)
- [Interledger.rs Architecture](./docs/architecture.md)
- [Interledger Forum](https://forum.interledger.org) for general questions about the Interledger Protocol and Project

## Installation
- Install Rust (using [`rustup`](https://rustup.rs/) is recommended)
- `cargo install interledger`

## Usage

### Run moneyd

`interledger moneyd local`

### Running an SPSP Server

#### Using BTP

`interledger spsp server --port 3000`

(You can see the full options by running `ilp spsp server --help`)

#### Using ILP-Over-HTTP

`interledger spsp server --use_ilp_over_http --ilp_address=private.local --incoming_auth_token=somesecrettoken`

### Sending an SPSP Payment

#### Using BTP

`interledger spsp pay --receiver=http://localhost:3000/spsp --amount=1000`

(You can see the full options by running `ilp spsp pay --help`)

#### Using ILP-Over-HTTP

`interledger spsp pay --receiver=http://localhost:3000/spsp --amount=1000 --http_server=http://localhost:3000/ilp`

## Testing

### Unit Tests

`cargo test`

### Integration Tests

Make sure `redis-server` is installed and available in your `PATH`.

`cargo test --all-features`
