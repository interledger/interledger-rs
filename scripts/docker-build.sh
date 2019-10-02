#!/bin/bash

# intended to be run in the top directory
docker build -f ./docker/node.dockerfile -t interledgerrs/node:latest .
docker build -f ./docker/eth-se.dockerfile -t interledgerrs/settlement-engine:latest .
docker build -f ./docker/Dockerfile -t interledgerrs/testnet-bundle:latest .
docker build -f ./docker/ilp-cli.dockerfile -t interledgerrs/ilp-cli:latest .
