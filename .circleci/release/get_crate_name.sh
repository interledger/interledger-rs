#!/usr/bin/env bash

# Returns a crate name to build from a git tag name.
# The tag name is assumed to be output by `cargo release` or be tagged manually.

# The regex means "(something)-(semantic version)"
if [[ $1 =~ ^(.*)-v([0-9]+)\.([0-9]+)\.([0-9]+)(-([0-9A-Za-z-]+(\.[0-9A-Za-z-]+)*))?(\+[0-9A-Za-z-]+)?$ ]]; then
    echo ${BASH_REMATCH[1]}
elif [[ $1 =~ ^ilp-node-.*$ ]]; then
    echo "ilp-node"
elif [[ $1 =~ ^ilp-cli-.*$ ]]; then
    echo "ilp-cli"
fi
