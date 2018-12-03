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

`ilp spsp server --port 3000`

(You can see the full options by running `ilp spsp server --help`)

### Sending an SPSP Payment

`ilp spsp pay --receiver http://localhost:3000/bob --amount 1000`

(You can see the full options by running `ilp spsp pay --help`)
