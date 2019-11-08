#!/usr/bin/env bash

# Build musl binary inside a Docker container and copy it to local disk.
# This script requires docker command.
# `crate_name` ($1) is expected to be the same as bin name.
#
# 1) Spin up a new container of "clux/muslrust:stable"
# 2) Copy source code etc. to the container
# 3) Build a musl binary of the specified crate ($1) inside the container
# 4) Copy the built binary to local disk space ($2)

crate_name=$1
artifacts_path=$2
docker_image_name="clux/muslrust:stable"

if [ -z "${crate_name}" ]; then
    printf "%s\n" "crate_name is required."
    exit 1
fi
if [ -z "${artifacts_path}" ]; then
    printf "%s\n" "artifacts_path is required."
    exit 1
fi

docker run -dt --name builder "${docker_image_name}"

docker cp ./Cargo.toml builder:/usr/src/Cargo.toml
docker cp ./crates builder:/usr/src/crates
docker cp ./.circleci/release/build.sh builder:/usr/src/build.sh

# "--workdir" requires API version 1.35, but the Docker daemon API version of CircleCI is 1.32
echo "cd /usr/src/; ./build.sh x86_64-unknown-linux-musl \"${crate_name}\"" | docker exec -i builder /bin/bash
docker cp "builder:/usr/src/target/x86_64-unknown-linux-musl/release/${crate_name}" "${artifacts_path}"

docker stop builder
