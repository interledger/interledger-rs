# Just to test markdown files
# Deploy compiled binary
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
COPY \
    target/x86_64-unknown-linux-musl/debug/interledger \
    /usr/local/bin/interledger

WORKDIR /opt/app

# ENV RUST_BACKTRACE=1
ENV RUST_LOG=interledger=debug

ENTRYPOINT [ "/usr/local/bin/interledger" ]
CMD [ "node" ]
