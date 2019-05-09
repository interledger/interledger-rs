# Build Node.js into standalone binaries
FROM node:11 as node

RUN npm install -g nexe@^3.0.0-beta.15

ENV PLATFORM alpine
ENV ARCH x64
ENV NODE 10.15.3

# Build settlement engine
WORKDIR /usr/src/settlement-engines/xrp
COPY ./settlement-engines /usr/src/settlement-engines
RUN npm install
RUN npm run build
RUN nexe \
    --target ${PLATFORM}-${ARCH}-${NODE} \
    ./build/cli.js \
    --output \
    /usr/local/bin/xrp-settlement-engine \
    --resource \
    "./scripts/*.lua"

# Build run script
WORKDIR /usr/src
COPY ./run-interledger-node.js ./run-interledger-node.js
RUN nexe \
    --target ${PLATFORM}-${ARCH}-${NODE} \
    ./run-interledger-node.js \
    --output \
    /usr/local/bin/run-interledger-node

# Build Interledger node into standalone binary
FROM clux/muslrust as rust

WORKDIR /usr/src
COPY ./Cargo.toml /usr/src/Cargo.toml
COPY ./crates /usr/src/crates

# TODO build release
RUN cargo build --package interledger


# Copy the binaries into the final stage
FROM alpine:latest

# Expose ports for HTTP and BTP
EXPOSE 7768
EXPOSE 7770

VOLUME [ "/data" ]
ENV REDIS_DIR=/data

WORKDIR /usr/local/bin

# Install SSL certs and Redis
RUN apk --no-cache add \
    ca-certificates \
    redis

# Copy in Node.js bundles
COPY --from=node \
    /usr/local/bin/xrp-settlement-engine \
    /usr/local/bin/xrp-settlement-engine
COPY --from=node \
    /usr/local/bin/run-interledger-node \
    /usr/local/bin/run-interledger-node

# Copy Interledger binary
COPY --from=rust \
    /usr/src/target/x86_64-unknown-linux-musl/debug/interledger \
    /usr/local/bin/interledger

CMD ["run-interledger-node"]