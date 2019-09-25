# Build Interledger node into standalone binary
FROM clux/muslrust:stable as rust

WORKDIR /usr/src
COPY ./Cargo.toml /usr/src/Cargo.toml
COPY ./crates /usr/src/crates

# TODO: investigate using a method like https://whitfin.io/speeding-up-rust-docker-builds/
# to ensure that the dependencies are cached so the build doesn't take as long
RUN cargo build --release --package interledger
# RUN cargo build --package interledger

FROM alpine

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
    /usr/src/target/x86_64-unknown-linux-musl/release/interledger \
    /usr/local/bin/interledger
# COPY --from=rust \
#     /usr/src/target/x86_64-unknown-linux-musl/debug/interledger \
#     /usr/local/bin/interledger

WORKDIR /opt/app

COPY ./docker/redis.conf redis.conf
COPY ./docker/run-node-and-redis.sh run-node-and-redis.sh

# ENV RUST_BACKTRACE=1
ENV RUST_LOG=interledger/.*

# In order for the node to access the config file, you need to mount
# the directory with the node's config.yml file as a Docker volume
# called "/config". You can do this by adding the option
# `-v /path/to/config.yml:/config` when calling `docker run`.
VOLUME [ "/config" ]

ENTRYPOINT [ "/bin/sh", "./run-node-and-redis.sh" ]
CMD [ "/config/config.yml" ]
