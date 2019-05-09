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

2. Create a docker volume to store Interledger node data:

    `docker volume create interledger-node-data`

3. Run the Interledger node **with the environment variables listed below***:

    `docker run --mount source=interledger-node-data,destination=/data emschwartz/interledger-rs`

| Environment Variable | Required? | Description |
|---|---|---|
| `XRP_ADDRESS` | Y | XRP account address to use for settlement |
| `XRP_SECRET` | Y | XRP accont secret to use for settlement |
| `ADMIN_TOKEN` | Y | HTTP Bearer token for admin account |
| `DEBUG` | N | Passed through to Node.js settlement engine. Set to `"*"` to see debug output |
| `RUST_LOG ` | N | Passed through to Rust components. Set to `"interledger/.*"` to see debug output |

\* Note that these can be set by prepending `XRP_ADDRESS=<address> XRP_SECRET=...` to the command.

4. Access your node via HTTP on port 7770 **of the Docker container's IP address** (not localhost -- try [`172.17.0.2`](http://172.17.0.2:7770) if you aren't running other containers)


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