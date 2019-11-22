#!/bin/bash

# intended to be run in the top directory
docker_image_tag=${DOCKER_IMAGE_TAG:-latest}

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
    build_targets+=(ilp-node)
    build_targets+=(testnet-bundle)
fi

# if arguments are given, add it as build targets
build_targets+=($@)

printf "\e[32;1mBuilding docker image with tag: ${docker_image_tag}\e[m\n"

for build_target in "${build_targets[@]}"; do
    case $build_target in
        "ilp-cli") docker build -f ./docker/ilp-cli.dockerfile -t interledgerrs/ilp-cli:${docker_image_tag} . ;;
        "ilp-node") docker build -f ./docker/ilp-node.dockerfile -t interledgerrs/ilp-node:${docker_image_tag} \
                --build-arg CARGO_BUILD_OPTION="${CARGO_BUILD_OPTION}" \
                --build-arg RUST_BIN_DIR_NAME="${RUST_BIN_DIR_NAME}" \
                . ;;
        "testnet-bundle") docker build -f ./docker/Dockerfile -t interledgerrs/testnet-bundle:${docker_image_tag} . ;;
    esac
done
