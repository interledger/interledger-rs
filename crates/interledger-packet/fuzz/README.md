# interledger-packet fuzzing

## Quickstart (cargo-fuzz)

See book for more information: https://rust-fuzz.github.io/book/cargo-fuzz.html

```
cargo install cargo-fuzz
```

Then under the interledger-packet root:

```
cargo +nightly fuzz run prepare
```
