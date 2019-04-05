<p align="center">
  <img src="interledger-rs.svg" width="700" alt="Interledger.rs">
</p>

---
> Interledger implementation in Rust :money_with_wings:

[![crates.io](https://img.shields.io/crates/v/interledger.svg)](https://crates.io/crates/interledger)
[![Interledger.rs Documentation](https://docs.rs/interledger/badge.svg)](https://docs.rs/interledger)
[![CircleCI](https://circleci.com/gh/emschwartz/interledger-rs.svg?style=shield)](https://circleci.com/gh/emschwartz/interledger-rs)
[![codecov](https://codecov.io/gh/emschwartz/interledger-rs/branch/master/graph/badge.svg)](https://codecov.io/gh/emschwartz/interledger-rs)
[![Docker Image](https://img.shields.io/docker/pulls/emschwartz/interledger-rs.svg?maxAge=2592000)](https://hub.docker.com/r/emschwartz/interledger-rs/)

## Understanding Interledger.rs
- [API Docs](https://docs.rs/interledger)
- [Interledger.rs Architecture](./docs/architecture.md)
- [Interledger Forum](https://forum.interledger.org) for general questions about the Interledger Protocol and Project

## Quickstart

### Docker Install

Prerequisites:
- Docker

Install and run:

1. Get the docker image:

    `docker pull emschwartz/interledger-rs`

2. Run the Interledger node

    `docker run -i -t -v interledger-node-data:/data emschwartz/interledger-rs`

3. Follow the instructions written to the terminal to try sending an SPSP payment


### Manual Install

Prerequisites:
- Git
- Rust (using [`rustup`](https://rustup.rs/) is recommended)
- Node.js - only to run the XRP settlement engine (using [`nvm`](https://github.com/creationix/nvm) is recommended)

Install and Run:

1. `git clone https://github.com/emschwartz/interledger-rs && cd interledger-rs`
2. `cargo build` (add `--release` to compile the release version, which is slower to compile but faster to run)
2. `cargo run --package interledger` (append command line options after a `--` to use the CLI)

## Contributing

Contributions are very welcome and if you're interested in getting involved, see [CONTRIBUTING.md](docs/CONTRIBUTING.md).