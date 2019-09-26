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

## Installation and Usage

### Docker

Prerequisites:
- Docker

Install

1. `docker pull interledgerrs/node`

Run

1. `docker run -it interledgerrs/node`

### Building From Source

Prerequisites:
- Git
- [Rust](https://www.rust-lang.org/tools/install) - latest stable version

Install

1. `git clone https://github.com/interledger-rs/interledger-rs && cd interledger-rs`
2. `cargo build` (add `--release` to compile the release version, which is slower to compile but faster to run)

Run

`cargo run`

Append the `--help` flag to see available options.

### Configuration

Interledger.rs commands such as `node` and `ethereum-ledger` accept configuration options in the following ways:

1. Environment variables
1. Standard In (stdin)
1. Configuration files
1. Command line arguments

The priority is: Environment Variables > stdin > configuration files > command line arguments.

```bash #
# 1.
# Passing by command line arguments.
# --{parameter name} {value}
cargo run -- --admin_auth_token super-secret

# 2.
# Passing by a configuration file in JSON, TOML, YAML format.
# The first argument after subcommands such as `node` is the path to the configuration file.
# Note that in order for a docker image to have access to a local file, it must be included in
# a directory that is mounted as a Volume at `/config`
cargo run -- node config.yml

# 3.
# Passing from STDIN in JSON, TOML, YAML format.
some_command | cargo run -- node

# 4.
# passing as environment variables
# {parameter name (typically in capital)}={value}
# note that the parameter names MUST begin with a prefix of "ILP_" e.g. ILP_SECRET_SEED
ILP_ADDRESS=example.alice \
ILP_OTHER_PARAMETER=other_value \
cargo run
```

## Examples

See the [examples](./examples/README.md) for demos of Interledger functionality and how to use the Interledger.rs implementation.

## Contributing

Contributions are very welcome and if you're interested in getting involved, see [CONTRIBUTING.md](docs/CONTRIBUTING.md). We're more than happy to answer questions and mentor you in making your first contributions to Interledger.rs!
