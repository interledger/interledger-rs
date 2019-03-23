FROM rust:1.33 as rust

WORKDIR /usr/src
COPY ./Cargo.toml /usr/src/Cargo.toml
COPY ./crates /usr/src/crates

# Build Interledger node
# TODO build release
RUN cargo build --package interledger

FROM node:11-slim as node

# Install Redis
RUN apt-get update \
    && apt-get -y install redis-server

# Build settlement engine
COPY ./settlement-engines /usr/src/settlement-engines
WORKDIR /usr/src/settlement-engines/xrp
RUN npm run build
WORKDIR /usr/src

# Copy Interledger binary
COPY --from=rust /usr/src/target/debug/interledger /usr/src/target/debug/interledger

# Expose ports for HTTP and BTP
EXPOSE 7768
EXPOSE 7770

COPY ./run-node.js ./run-node.js

VOLUME [ "/data" ]
ENV REDIS_DIR=/data

CMD ["node", "./run-node.js"]