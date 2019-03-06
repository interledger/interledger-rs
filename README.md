<p align="center">
  <img src="interledger-rs.svg" width="700" alt="Interledger.rs">
</p>

---
> Interledger implementation in Rust :money_with_wings:

[![crates.io](https://img.shields.io/crates/v/interledger.svg)](https://crates.io/crates/interledger)
[![CircleCI](https://circleci.com/gh/emschwartz/interledger-rs.svg?style=shield)](https://circleci.com/gh/emschwartz/interledger-rs)
[![codecov](https://codecov.io/gh/emschwartz/interledger-rs/branch/master/graph/badge.svg)](https://codecov.io/gh/emschwartz/interledger-rs)

## Installation
- Install Rust (using [`rustup`](https://rustup.rs/) is recommended)
- Install [`moneyd`](https://github.com/interledgerjs/moneyd)
- `cargo install interledger`

## Usage

Make sure you're running `moneyd` first! (`moneyd local` can be used for testing purposes)

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
