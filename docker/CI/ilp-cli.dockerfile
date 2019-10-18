# Just to test markdown files
# Deploy compiled binary
FROM alpine

# Install SSL certs
RUN apk --no-cache add ca-certificates

# Copy ilp-cli binary
COPY \
    target/x86_64-unknown-linux-musl/debug/ilp-cli \
    /usr/local/bin/ilp-cli

WORKDIR /opt/app

# ENV RUST_BACKTRACE=1
ENV RUST_LOG=interledger=debug

ENTRYPOINT [ "/usr/local/bin/ilp-cli" ]
