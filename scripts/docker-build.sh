#!/bin/bash

# intended to be run in the top directory
docker build -f ./docker/node.dockerfile -t interledgerrs/node:latest .
docker build -f ./docker/eth-se.dockerfile -t interledgerrs/settlement-engine:latest .
