#!/bin/bash

# turn on bash's job control
set -m

redis-server --dir /data &
lt -s test -p 7770 &
DEBUG=settlement* ilp-settlement-xrp &
# interledger-settlement-engines &
sleep 1
ilp-node $@
