# Build Interledger node into standalone binary
FROM clux/muslrust:stable as rust

WORKDIR /usr/src
COPY ./Cargo.toml /usr/src/Cargo.toml
COPY ./crates /usr/src/crates

RUN cargo build --release --package interledger
# RUN cargo build --package interledger

# Deploy compiled binary to another container
FROM alpine

# Expose ports for HTTP and BTP
# - 7768: BTP
# - 7770: HTTP - ILP over HTTP, API
# - 7771: HTTP - settlement
EXPOSE 7768
EXPOSE 7770
EXPOSE 7771

# Install SSL certs
RUN apk --no-cache add ca-certificates

# Copy Interledger binary
COPY --from=rust \
    /usr/src/target/x86_64-unknown-linux-musl/release/interledger \
    /usr/local/bin/interledger
# COPY --from=rust \
#     /usr/src/target/x86_64-unknown-linux-musl/debug/interledger \
#     /usr/local/bin/interledger

WORKDIR /opt/app

# ENV RUST_BACKTRACE=1
ENV RUST_LOG=interledger=debug

ENTRYPOINT [ "/usr/local/bin/interledger" ]
CMD [ "node" ]
