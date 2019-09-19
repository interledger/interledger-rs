# Just to test markdown files
# Deploy compiled binary
FROM alpine

# Expose ports for HTTP
# - 3000: HTTP - settlement
EXPOSE 3000

# Install SSL certs
RUN apk --no-cache add ca-certificates

# Copy Interledger binary
COPY \
    target/x86_64-unknown-linux-musl/debug/interledger-settlement-engines \
    /usr/local/bin/interledger-settlement-engines

WORKDIR /opt/app

# ENV RUST_BACKTRACE=1
ENV RUST_LOG=interledger=debug

ENTRYPOINT [ "/usr/local/bin/interledger-settlement-engines" ]
CMD [ "ethereum-ledger" ]
