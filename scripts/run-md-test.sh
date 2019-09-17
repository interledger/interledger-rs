#!/bin/bash

function init_test() {
    clear_logs
    clear_docker
    free_ports
    clear_redis_file
}

function clear_logs() {
    rm -rf logs
}

function clear_docker() {
    docker network rm interledger 2>/dev/null
    docker stop $(docker ps -aq) 2>/dev/null
    docker rm $(docker ps -aq) 2>/dev/null
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

# $1 = target directory
# $2 = docker mode in [01]
#
# test_example examples/eth-settlement 0
function test_example() {
    local target_directory=$1
    local docker_mode=$2
    local target_name=$(basename $target_directory)

    cd $target_directory
    if [ $docker_mode -eq 1 ]; then
        printf "\e[33;1m%b [%d/%d]\e[m\n" "Testing \"${target_name}\" on docker mode." "$((TESTING_INDEX + 1))" "${TESTS_TOTAL}"
        init_test; USE_DOCKER=1 $RUN_MD .
        return $?
    else
        printf "\e[33;1m%b [%d/%d]\e[m\n" "Testing \"${target_name}\" on non-docker mode." "$((TESTING_INDEX + 1))" "${TESTS_TOTAL}"
        init_test; $RUN_MD .
        return $?
    fi
}

# Adds all directories in `examples` directory as test targets.
function add_example_tests() {
    # This will add tests like "test_example eth-settlement 0" which means an example test of
    # eth-settlement in non-docker mode.
    for directory in $(find $BASE_DIR/../examples/* -type d -maxdepth 0); do
        TESTS+=("test_example ${directory} 0") # this cannot contain space in the directory path
        TESTS+=("test_example ${directory} 1")
    done
}

BASE_DIR=$(cd $(dirname $0); pwd)
RUN_MD=$BASE_DIR/run-md.sh

TESTS=()
add_example_tests
TESTS_TOTAL=${#TESTS[@]}
TESTS_FAILED=0
TESTING_INDEX=0
for test in "${TESTS[@]}"; do
    TEST_MODE=1 $test
    if [ $? -ne 0 ]; then
        TESTS_FAILED=$((TESTS_FAILED + 1))
    fi
    TESTING_INDEX=$((TESTING_INDEX + 1))
done
if [ $TESTS_FAILED -eq 0 ]; then
    printf "\e[32;1m%b [%d/%d]\e[m\n" "All tests passed!" "${TESTS_TOTAL}" "${TESTS_TOTAL}"
    exit 0
else
    printf "\e[31;1m%b [%d/%d]\e[m\n" "Some tests failed." "${TESTS_FAILED}" "${TESTS_TOTAL}"
    exit 1
fi
