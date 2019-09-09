# Build Interledger node into standalone binary
FROM clux/muslrust:stable as rust

WORKDIR /usr/src
COPY ./Cargo.toml /usr/src/Cargo.toml
COPY ./crates /usr/src/crates

RUN cargo build --release --package interledger-settlement-engines
# RUN cargo build --package interledger-settlement-engines

# Deploy compiled binary to another container
FROM alpine

# Expose ports for HTTP
# - 3000: HTTP - settlement
EXPOSE 3000

# Install SSL certs
RUN apk --no-cache add ca-certificates

# Copy Interledger binary
COPY --from=rust \
    /usr/src/target/x86_64-unknown-linux-musl/release/interledger-settlement-engines \
    /usr/local/bin/interledger-settlement-engines
# COPY --from=rust \
#     /usr/src/target/x86_64-unknown-linux-musl/debug/interledger-settlement-engines \
#     /usr/local/bin/interledger-settlement-engines

WORKDIR /opt/app

# ENV RUST_BACKTRACE=1
ENV RUST_LOG=interledger=debug

ENTRYPOINT [ "/usr/local/bin/interledger-settlement-engines" ]
CMD [ "ethereum-ledger" ]
