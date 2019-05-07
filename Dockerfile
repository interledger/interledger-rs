# Build Interledger node into standalone binary
FROM clux/muslrust as rust

WORKDIR /usr/src
COPY ./Cargo.toml /usr/src/Cargo.toml
COPY ./crates /usr/src/crates

# TODO build release
RUN cargo build --package interledger

FROM node:11-alpine

# Expose ports for HTTP and BTP
EXPOSE 7768
EXPOSE 7770

VOLUME [ "/data" ]
ENV REDIS_DIR=/data

# Install SSL certs and Redis
RUN apk --no-cache add \
    ca-certificates \
    redis

# Install localtunnel
RUN npm install localtunnel

# Build run script
WORKDIR /usr/src
COPY ./run-interledger-node.js ./run-interledger-node.js

# Copy Interledger binary
COPY --from=rust \
    /usr/src/target/x86_64-unknown-linux-musl/debug/interledger \
    /usr/local/bin/interledger

CMD ["node", "./run-interledger-node.js"]