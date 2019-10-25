FROM circleci/rust:1.38

# Make sure you have copied macOS SDK package in the top directory.
# https://github.com/tpoechtrager/osxcross#packaging-the-sdk-on-mac-os-x
# MacOSX10.15.sdk.tar.xz

USER root

RUN apt-get update && \
    apt-get install --yes \
    clang \
    cmake

WORKDIR /opt
RUN git clone https://github.com/tpoechtrager/osxcross
COPY MacOSX10.15.sdk.tar.xz osxcross/tarballs

WORKDIR osxcross
RUN UNATTENDED=1 ./build.sh

ENV PATH=/opt/osxcross/target/bin:$PATH \
    CC=o64-clang \
    CXX=o64-clang++

RUN rustup target add x86_64-apple-darwin

WORKDIR /home/circleci
ENV CARGO_HOME=/home/circleci/.cargo
RUN mkdir -p ${CARGO_HOME} && \
    echo '[target.x86_64-apple-darwin] \n\
linker = "x86_64-apple-darwin19-cc" \n\
ar = "x86_64-apple-darwin19-ar"' >> ${CARGO_HOME}/config

ENTRYPOINT [ "/bin/bash" ]
CMD [ ]
