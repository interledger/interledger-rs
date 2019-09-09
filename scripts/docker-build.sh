#!/bin/bash

docker build -f node.dockerfile -t interledgerrs/node:latest .
docker build -f eth-se.dockerfile -t interledgerrs/settlement-engine:latest .
