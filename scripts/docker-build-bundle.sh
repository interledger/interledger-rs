#!/bin/bash

# intended to be run in the top directory
sudo docker build -f ./docker/Dockerfile -t interledgerrs/testnet-bundle:latest .
