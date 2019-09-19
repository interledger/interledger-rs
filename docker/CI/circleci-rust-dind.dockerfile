# rust + dind + musl + (nvm + npm + ganache + settlement-xrp)
# rust:latest @ 2019/09/17
FROM circleci/rust@sha256:3e7f62299c1b6fa3a429ddfb19b8d43bcd92c329b9b604571801a8d2273d16f6

USER root

# Install libcurl3 etc.
# cargo-audit started requiring libcurl3
RUN echo "deb http://security.ubuntu.com/ubuntu xenial-security main" | tee -a /etc/apt/sources.list && \
    apt-key adv --recv-keys --keyserver keyserver.ubuntu.com 3B4FE6ACC0B21F32 && \
    apt-get update && \
    apt-get install libcurl3 -y && \
    # get libcurl to a place where it won't get overwritten
    cp /usr/lib/x86_64-linux-gnu/libcurl.so.3 /usr/lib && \
    apt-get install curl -y

# Because nc command doesn't accept -k argument correctly, we need to install ncat of buster.
RUN echo "deb http://deb.debian.org/debian/ buster main contrib non-free" >> /etc/apt/sources.list && \
    apt-get update && \
    apt-get install -y ncat redis-server redis-tools lsof libssl-dev

# Install Rust tools
RUN cargo install --quiet cargo-audit && \
    rustup component add rustfmt && \
    rustup component add clippy && \
    rustup target add x86_64-unknown-linux-musl && \
    rustup show

# Build musl and openssl
# https://github.com/clux/muslrust/blob/master/Dockerfile
# Using export not to affect env vars of CircleCI.
# Affecting these env vars causes compilation errors in CircleCI.
WORKDIR /opt/musl
RUN export SSL_VER="1.0.2s" && \
    export CC=musl-gcc && \
    export PREFIX=/opt/musl && \
    export LD_LIBRARY_PATH=$PREFIX && \
    echo "$PREFIX/lib" >> /etc/ld-musl-x86_64.path && \
    ln -s /usr/include/x86_64-linux-gnu/asm /usr/include/x86_64-linux-musl/asm && \
    ln -s /usr/include/asm-generic /usr/include/x86_64-linux-musl/asm-generic && \
    ln -s /usr/include/linux /usr/include/x86_64-linux-musl/linux
RUN export SSL_VER="1.0.2s" && \
    export CC=musl-gcc && \
    export PREFIX=/opt/musl && \
    export LD_LIBRARY_PATH=$PREFIX && \
    curl -sSL https://www.openssl.org/source/openssl-$SSL_VER.tar.gz | tar xz && \
    cd openssl-$SSL_VER && \
    ./Configure no-zlib no-shared -fPIC --prefix=$PREFIX --openssldir=$PREFIX/ssl linux-x86_64 && \
    env C_INCLUDE_PATH=$PREFIX/include make depend 2> /dev/null && \
    make -j$(nproc) && make install && \
    cd .. && rm -rf openssl-$SSL_VER

USER circleci
WORKDIR /home/circleci

# Install nvm, node and ganache-cli
# node v12 causes a compilation error
RUN curl -o- https://raw.githubusercontent.com/nvm-sh/nvm/v0.34.0/install.sh | bash
ENV NVM_DIR="/home/circleci/.nvm"
ENV NODE_VERSION="v11.15.0"
ENV NODE_PATH=$NVM_DIR/versions/node/$NODE_VERSION/lib/node_modules
ENV PATH=$NVM_DIR/versions/node/$NODE_VERSION/bin:$PATH
RUN . ~/.nvm/nvm.sh && \
    nvm install $NODE_VERSION && \
    npm install -g ganache-cli

# Build and install settlement-xrp
RUN git clone https://github.com/interledgerjs/settlement-xrp/
WORKDIR settlement-xrp
RUN . ~/.nvm/nvm.sh && \
    npm install graphql@0.11.3 && \
    npm install && \
    npm run build && \
    npm link

WORKDIR /home/circleci
ENV CARGO_HOME=/home/circleci/.cargo
ENV OPENSSL_DIR=/opt/musl

ENTRYPOINT [ "/bin/bash" ]
CMD [ ]
