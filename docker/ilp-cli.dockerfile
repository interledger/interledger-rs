# Build Interledger node into standalone binary
FROM clux/muslrust:stable as rust

WORKDIR /usr/src
COPY ./Cargo.toml /usr/src/Cargo.toml
COPY ./crates /usr/src/crates

# TODO: investigate using a method like https://whitfin.io/speeding-up-rust-docker-builds/
# to ensure that the dependencies are cached so the build doesn't take as long
# RUN cargo build --all-features --package ilp-node --package interledger-settlement-engines --package ilp-cli
RUN cargo build --release --all-features --package ilp-cli

FROM alpine

# Expose ports for HTTP server
EXPOSE 7770

# Install SSL certs
RUN apk --no-cache add \
    ca-certificates

# Copy Interledger binary
COPY --from=rust \
    /usr/src/target/x86_64-unknown-linux-musl/release/ilp-cli \
    /usr/local/bin/ilp-cli

ENTRYPOINT [ "ilp-cli" ]
