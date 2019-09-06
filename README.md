<p align="center">
  <img src="docs/interledger-rs.svg" width="700" alt="Interledger.rs">
</p>

---
> Interledger implementation in Rust :money_with_wings:

[![crates.io](https://img.shields.io/crates/v/interledger.svg)](https://crates.io/crates/interledger)
[![Interledger.rs Documentation](https://docs.rs/interledger/badge.svg)](https://docs.rs/interledger)
[![CircleCI](https://circleci.com/gh/interledger-rs/interledger-rs.svg?style=shield)](https://circleci.com/gh/interledger-rs/interledger-rs)
![Rust Version](https://img.shields.io/badge/rust-stable-Success)
[![Docker Image](https://img.shields.io/docker/pulls/interledgerrs/node.svg?maxAge=2592000)](https://hub.docker.com/r/interledgerrs/node/)

## Understanding Interledger.rs
- [HTTP API](./docs/api.md)
- [Rust API](https://docs.rs/interledger)
- [Interledger.rs Architecture](./docs/architecture.md)
- [Interledger Forum](https://forum.interledger.org) for general questions about the Interledger Protocol and Project

## Installation

Prerequisites:
- Git
- [Rust](https://www.rust-lang.org/tools/install) - latest stable version

Install and Run:

1. `git clone https://github.com/interledger-rs/interledger-rs && cd interledger-rs`
2. `cargo build` (add `--release` to compile the release version, which is slower to compile but faster to run)
2. `cargo run` (append command line options after a `--` to use the CLI)

## Running the Examples

See the [examples](./examples/README.md) for demos of Interledger functionality and how to use the Interledger.rs implementation.

## Contributing

Contributions are very welcome and if you're interested in getting involved, see [CONTRIBUTING.md](docs/CONTRIBUTING.md). We're more than happy to answer questions and mentor you in making your first contributions to Interledger.rs!
