#!/bin/bash

build_targets=()

if [ "$PROFILE" = "release" ]; then
    printf "\e[32;1mBuilding profile: release\e[m\n"
    CARGO_BUILD_OPTION=--release
    RUST_BIN_DIR_NAME=release
else
    printf "\e[32;1mBuilding profile: dev\e[m\n"
    CARGO_BUILD_OPTION=
    RUST_BIN_DIR_NAME=debug
fi

if [ $# -eq 0 ]; then
    printf "\e[33mNo build target is given, building all targets.\e[m\n"
    build_targets+=(ilp-cli)
    build_targets+=(node)
    build_targets+=(settlement-engines)
    build_targets+=(testnet-bundle)
    build_targets+=(circleci-rust-dind)
fi

# if arguments are given, add it as build targets
build_targets+=($@)

for build_target in "${build_targets[@]}"; do
    case $build_target in
        # intended to be run in the top directory
        "ilp-cli") docker build -f ./docker/ilp-cli.dockerfile -t interledgerrs/ilp-cli:latest . ;;
        "node") docker build -f ./docker/node.dockerfile -t interledgerrs/node:latest \
                --build-arg CARGO_BUILD_OPTION="${CARGO_BUILD_OPTION}" \
                --build-arg RUST_BIN_DIR_NAME="${RUST_BIN_DIR_NAME}" \
                . ;;
        "settlement-engines") docker build -f ./docker/settlement-engines.dockerfile -t interledgerrs/settlement-engines:latest \
                --build-arg CARGO_BUILD_OPTION="${CARGO_BUILD_OPTION}" \
                --build-arg RUST_BIN_DIR_NAME="${RUST_BIN_DIR_NAME}" \
                . ;;
        "testnet-bundle") docker build -f ./docker/Dockerfile -t interledgerrs/testnet-bundle:latest . ;;
        "circleci-rust-dind") docker build -f ./docker/circleci-rust-dind.dockerfile -t interledgerrs/circleci-rust-dind:latest . ;;
    esac
done
