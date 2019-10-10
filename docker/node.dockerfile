# Build Interledger node into standalone binary
FROM clux/muslrust:stable as rust

WORKDIR /usr/src
COPY ./Cargo.toml /usr/src/Cargo.toml
COPY ./crates /usr/src/crates

RUN cargo build --release --package ilp-node
# RUN cargo build --package ilp-node

# Deploy compiled binary to another container
FROM alpine

# Expose ports for HTTP and BTP
# - 7770: HTTP - ILP over HTTP, API
# - 7771: HTTP - settlement
EXPOSE 7770
EXPOSE 7771

# Install SSL certs
RUN apk --no-cache add ca-certificates

# Copy Interledger binary
COPY --from=rust \
    /usr/src/target/x86_64-unknown-linux-musl/release/ilp-node \
    /usr/local/bin/ilp-node
# COPY --from=rust \
#     /usr/src/target/x86_64-unknown-linux-musl/debug/ilp-node \
#     /usr/local/bin/ilp-node

WORKDIR /opt/app

# ENV RUST_BACKTRACE=1
ENV RUST_LOG=ilp,interledger=debug

ENTRYPOINT [ "/usr/local/bin/ilp-node" ]
