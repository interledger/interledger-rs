#!/bin/bash

# this script could be more generalized but just for now
cat $1 | node -e 'process.stdout.write(JSON.parse(require("fs").readFileSync("/dev/stdin", "utf8")).id);'
