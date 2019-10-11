# Build Interledger node into standalone binary
FROM clux/muslrust:stable as rust

WORKDIR /usr/src/interledger-rs
COPY ./Cargo.toml /usr/src/interledger-rs/Cargo.toml
COPY ./crates /usr/src/interledger-rs/crates

# TODO: investigate using a method like https://whitfin.io/speeding-up-rust-docker-builds/
# to ensure that the dependencies are cached so the build doesn't take as long
# RUN cargo build --all-features --package ilp-node --package interledger-settlement-engines --package ilp-cli
RUN cargo build --release --all-features --package ilp-node --package ilp-cli

WORKDIR /usr/src/
RUN git clone https://github.com/interledger-rs/settlement-engines.git
WORKDIR /usr/src/settlement-engines
RUN cargo build --release --all-features --package interledger-settlement-engines

FROM node:12-alpine

# Expose ports for HTTP server
EXPOSE 7770

# To save the node's data across runs, mount a volume called "/data".
# You can do this by adding the option `-v data-volume-name:/data`
# when calling `docker run`.

VOLUME [ "/data" ]

# Install SSL certs and Redis
RUN apk --no-cache add \
    ca-certificates \
    redis

# Copy Interledger binary
COPY --from=rust \
    /usr/src/interledger-rs/target/x86_64-unknown-linux-musl/release/ilp-node \
    /usr/local/bin/ilp-node
COPY --from=rust \
    /usr/src/interledger-rs/target/x86_64-unknown-linux-musl/release/ilp-cli \
    /usr/local/bin/ilp-cli
COPY --from=rust \
    /usr/src/settlement-engines/target/x86_64-unknown-linux-musl/release/interledger-settlement-engines \
    /usr/local/bin/interledger-settlement-engines
# COPY --from=rust \
#     /usr/src/interledger-rs/target/x86_64-unknown-linux-musl/debug/ilp-node \
#     /usr/local/bin/ilp-node
# COPY --from=rust \
#     /usr/src/interledger-rs/target/x86_64-unknown-linux-musl/debug/ilp-cli \
#     /usr/local/bin/ilp-cli
# COPY --from=rust \
#     /usr/src/settlement-engines/target/x86_64-unknown-linux-musl/debug/interledger-settlement-engines \
#     /usr/local/bin/interledger-settlement-engines

WORKDIR /opt/app

RUN npm install -g ilp-settlement-xrp localtunnel

COPY ./docker/redis.conf redis.conf
COPY ./docker/run-testnet-bundle.js run-testnet-bundle.js

# ENV RUST_BACKTRACE=1
ENV RUST_LOG=interledger=debug

# In order for the node to access the config file, you need to mount
# the directory with the node's config.yml file as a Docker volume
# called "/config". You can do this by adding the option
# `-v /path/to/config.yml:/config` when calling `docker run`.
VOLUME [ "/config" ]

ENTRYPOINT [ "node", "run-testnet-bundle.js" ]
