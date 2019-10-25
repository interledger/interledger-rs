#!/usr/bin/env bash

# Just not to publish crates
# $ ./scripts/release.sh --dry-run
cargo release --skip-publish $@
