<p align="center">
  <img src="docs/interledger-rs.svg" width="700" alt="Interledger.rs">
</p>

---
> Interledger implementation in Rust :money_with_wings:

[![crates.io](https://img.shields.io/crates/v/interledger.svg)](https://crates.io/crates/interledger)
[![Interledger.rs Documentation](https://docs.rs/interledger/badge.svg)](https://docs.rs/interledger)
[![CircleCI](https://circleci.com/gh/interledger-rs/interledger-rs.svg?style=shield)](https://circleci.com/gh/interledger-rs/interledger-rs)
![rustc](https://img.shields.io/badge/rustc-1.39+-red.svg)
![Rust](https://img.shields.io/badge/rust-stable-Success)
[![Docker Image](https://img.shields.io/docker/pulls/interledgerrs/ilp-node.svg?maxAge=2592000)](https://hub.docker.com/r/interledgerrs/ilp-node/)
[![License](https://img.shields.io/badge/License-Apache%202.0-blue.svg)](https://opensource.org/licenses/Apache-2.0)

## Requirements

All crates require Rust 2018 edition and are tested on the following channels:

- `stable`

## Connecting to the Testnet

See the [testnet instructions](./docs/testnet.md) to quickly connect to the testnet with a bundle that includes the Interledger.rs node and settlement engines.

## Understanding Interledger.rs
- [HTTP API](./docs/api.md)
- [Rust API](https://docs.rs/interledger)
- [Interledger.rs Architecture](./docs/architecture.md)
- [Interledger Forum](https://forum.interledger.org) for general questions about the Interledger Protocol and Project

## Installation and Usage

To run the Interledger.rs components by themselves (rather than the `testnet-bundle`), you can follow these instructions:

### Using Docker

#### Prerequisites

- Docker

#### Install

```bash #
docker pull interledgerrs/ilp-node
docker pull interledgerrs/ilp-cli
docker pull interledgerrs/ilp-settlement-ethereum
```

#### Run

```bash #
# This runs the sender / receiver / router bundle
docker run -it interledgerrs/ilp-node

# This is a simple CLI for interacting with the node's HTTP API
docker run -it --rm interledgerrs/ilp-cli

# This includes the Ethereum Settlement Engines written in Rust
docker run -it interledgerrs/ilp-settlement-ethereum
```

### Building From Source

#### Prerequisites

- Git
- [Redis](https://redis.io/)
- [Rust](https://www.rust-lang.org/tools/install) - latest stable version

#### Install

```bash #
# 1. Clone the repsitory and change the working directory
git clone https://github.com/interledger-rs/interledger-rs && cd interledger-rs

# 2. Build interledger-rs (add `--release` to compile the release version, which is slower to compile but faster to run)
# This is temporary fix to OPENSSL build error.
export OPENSSL_INCLUDE_DIR=$PWD/openssl-1.1/include
export OPENSSL_LIB_DIR=$PWD/openssl-1.1/lib
cargo build
```

You can find the Interledger Settlement Engines in a [separate repository](https://github.com/interledger-rs/settlement-engines).

#### Run

```bash #
# This is temporary fix to OPENSSL build error.
export OPENSSL_INCLUDE_DIR=$PWD/openssl-1.1/include
export OPENSSL_LIB_DIR=$PWD/openssl-1.1/lib

# This runs the ilp-node
cargo run --bin ilp-node -- # Put CLI args after the "--"

cargo run --bin ilp-cli -- # Put CLI args after the "--"
```

Append the `--help` flag to see available options.

See [configuration](./docs/configuration.md) for more details on how the node is configured.

#### Configuring Redis

We have some account settings such as `amount_per_minute_limit` or `packets_per_minute_limit`. In order to enable these options, you need to load the [redis-cell](https://github.com/brandur/redis-cell) module as follows. *You don't need to load this module unless you use the rate-limit options.*

```
# in your redis config file
# libredis_cell.so file will be found in crates/interledger-store/external
loadmodule /path/to/modules/libredis_cell.so
```

or you can specify an argument when you start up the redis instance as follows.

```
redis-server --loadmodule /path/to/modules/libredis_cell.so
```

## Examples

See the [examples](./examples/README.md) for demos of Interledger functionality and how to use the Interledger.rs implementation.

## Contributing

Contributions are very welcome and if you're interested in getting involved, see [CONTRIBUTING.md](docs/CONTRIBUTING.md). We're more than happy to answer questions and mentor you in making your first contributions to Interledger.rs (even if you've never written in Rust before)!
