#!/bin/bash
#
# Runs the examples listed in the READMEs from `examples/*/README.md`.
#
# Requires additional runtime dependencies as listed in those READMEs.
#
# The code snippets and comments are parsed by `parse-md.sh` and can
# essentially be split into two sections:
#   1: binary aquisition
#   2: test execution
#
# (1) is controlled by the `SOURCE_MODE` parameter in the READMEs and
# is set by the second flag passed to this script ($2).
#
# Has two parameters:
#   $1 = test name filter
#   $2 = source mode flag
#      1: build binaries from source
#      2: use existing binaries (or download binaries if they don't exist)
#      *: download binaries (replaces existing binaries)


# $1: source mode flag
#    2: don't clear binaries
#    *: clear binaries
function clear_environment() {
    clear_logs
    free_ports
    clear_redis_file

    if [ ! $1 -eq 2 ]; then
        clear_binaries
    fi
}

function clear_logs() {
    rm -rf logs
}

function free_ports() {
    # ports of redis
    for port in $(seq 6379 6385); do
        if lsof -Pi :${port} -sTCP:LISTEN -t >/dev/null ; then
            redis-cli -p ${port} shutdown
        fi
    done

    # ports of ganache + node + se
    for port in 8545 7770 8770 9770 3000 3001 3002 3003; do
        if lsof -tPi :${port} >/dev/null ; then
            kill `lsof -tPi :${port}`
        fi
    done
}

function clear_redis_file() {
    # redis file
    if [ -f dump.rdb ] ; then
        rm -f dump.rdb
    fi
}

function clear_binaries() {
    rm -f ~/.interledger/bin/*
}

# $1 = target directory
# $2 = source mode
# $3 = test name filter
#
# test_example examples/eth-settlement 0
function test_example() {
    local target_directory=$1
    local source_mode=$2
    local target_name=$(basename $target_directory)
    local result

    pushd $target_directory > /dev/null
    if [ $source_mode -eq 1 ]; then
        printf "\e[33;1mTesting \"${target_name}\" on source mode. [%d/%d]\e[m\n" "$((TESTING_INDEX + 1))" "${TESTS_TOTAL}"
        clear_environment $2;
        TEST_MODE=1 SOURCE_MODE=1 $RUN_MD .
        result=$?
    else
        printf "\e[33;1mTesting \"${target_name}\" on binary mode. [%d/%d]\e[m\n" "$((TESTING_INDEX + 1))" "${TESTS_TOTAL}"
        clear_environment $2;
        TEST_MODE=1 $RUN_MD .
        result=$?
    fi
    mkdir -p ${LOG_DIR}/${target_name}
    mv logs ${LOG_DIR}/${target_name}/${source_mode}
    popd > /dev/null
    return $result
}

# Adds all directories in `examples` directory as test targets.
function add_example_tests() {
    local test_name_filter=$1
    local source_mode_filter=$2

    # This will add tests like "test_example examples/eth-settlement 0" which means an example test of
    # eth-settlement in non-docker mode.
    for directory in $(find $BASE_DIR/../examples/* -maxdepth 0 -type d); do
        if [ -n "${test_name_filter}" ] && [[ ! "${directory}" =~ ${test_name_filter} ]]; then
            printf "\e[33;1mSkipping \"%s\"\e[m\n" "${directory}"
            continue
        fi
        if [ -n "${source_mode_filter}" ]; then
            printf "\e[33;1mAdding only SOURCE_MODE=%d for \"%s\"\e[m\n" "${source_mode_filter}" "${directory}"
            TESTS+=("test_example ${directory} ${source_mode_filter}")
        else
            TESTS+=("test_example ${directory} 0") # this cannot contain space in the directory path
            TESTS+=("test_example ${directory} 1")
        fi
    done
}

# $1 = test name filter
# $2 = source mode flag

BASE_DIR=$(cd $(dirname $0); pwd)
RUN_MD=$BASE_DIR/run-md.sh
LOG_DIR=/tmp/run-md-test/logs

# set up example tests
TESTS=()
add_example_tests $@

# set up log dir for CircleCI
# the log files will be put as artifacts
rm -rf $LOG_DIR
mkdir -p $LOG_DIR

TESTS_TOTAL=${#TESTS[@]}
TESTS_FAILED=0
TESTING_INDEX=0
for test in "${TESTS[@]}"; do
    $test
    if [ $? -ne 0 ]; then
        TESTS_FAILED=$((TESTS_FAILED + 1))
    fi
    TESTING_INDEX=$((TESTING_INDEX + 1))
done
if [ $TESTS_FAILED -eq 0 ]; then
    printf "\e[32;1mAll tests passed! [%d/%d]\e[m\n" $((TESTS_TOTAL - TESTS_FAILED)) "${TESTS_TOTAL}"
    exit 0
else
    printf "\e[31;1mSome tests failed. [%d/%d]\e[m\n" $((TESTS_TOTAL - TESTS_FAILED)) "${TESTS_TOTAL}"
    exit 1
fi
