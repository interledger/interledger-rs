#!/usr/bin/env bash

# e.g. x86_64-unknown-linux-musl x86_64-apple-darwin
target_name=$1
# e.g. ilp-node
crate_name=$2

if [ -z "${target_name}" ]; then
    printf "%s\n" "target_name is required."
    exit 1
fi

if [ -z "${crate_name}" ]; then
    printf "%s\n" "crate_name is required."
    exit 1
fi

mkdir -p .cargo
printf "[build]\ntarget = \"${target_name}\"\n" >> .cargo/config
cargo build --release --package "${crate_name}" --bin "${crate_name}"
