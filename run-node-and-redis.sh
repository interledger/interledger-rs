#!/bin/bash

# turn on bash's job control
set -m

redis-server --dir /data &
sleep 1
interledger node $@
