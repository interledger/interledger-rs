#!/bin/bash

# Starts 2 redis instances at ports 6379 and 6380 for testing
# Kills whatever's already running at 6379 and 6380

if lsof -Pi :6379 -sTCP:LISTEN -t >/dev/null ; then
    redis-cli -p 6379 shutdown
fi
redis-server --port 6379 --daemonize yes &> /dev/null
redis-cli -p 6379 flushall

if lsof -Pi :6380 -sTCP:LISTEN -t >/dev/null ; then
    redis-cli -p 6380 shutdown
fi
redis-server --port 6380 --daemonize yes &> /dev/null
redis-cli -p 6380 flushall
