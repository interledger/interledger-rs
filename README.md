<p align="center">
  <img src="interledger-rs.svg" width="700" alt="Interledger.rs">
</p>

---
> Interledger implementation in Rust :money_with_wings:

[![crates.io](https://img.shields.io/crates/v/interledger.svg)](https://crates.io/crates/interledger)
[![Interledger.rs Documentation](https://docs.rs/interledger/badge.svg)](https://docs.rs/interledger)
[![CircleCI](https://circleci.com/gh/emschwartz/interledger-rs.svg?style=shield)](https://circleci.com/gh/emschwartz/interledger-rs)
<!-- [![Docker Image](https://img.shields.io/docker/pulls/emschwartz/interledger-rs.svg?maxAge=2592000)](https://hub.docker.com/r/emschwartz/interledger-rs/) -->

## Understanding Interledger.rs
- [HTTP API](./docs/api.md)
- [Rust API](https://docs.rs/interledger)
- [Interledger.rs Architecture](./docs/architecture.md)
- [Interledger Forum](https://forum.interledger.org) for general questions about the Interledger Protocol and Project

### Installation

Prerequisites:
- Git
- Rust, latest stable version (using [`rustup`](https://rustup.rs/) is recommended)

Install and Run:

1. `git clone https://github.com/emschwartz/interledger-rs && cd interledger-rs`
2. `cargo build` (add `--release` to compile the release version, which is slower to compile but faster to run)
2. `cargo run --package interledger` (append command line options after a `--` to use the CLI)

## Contributing

Contributions are very welcome and if you're interested in getting involved, see [CONTRIBUTING.md](docs/CONTRIBUTING.md). We're more than happy to answer questions and mentor you in making your first contributions to Interledger.rs!