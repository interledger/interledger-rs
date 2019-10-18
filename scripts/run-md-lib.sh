#!/bin/bash

# initialize global variables
function init() {
    if [ -n "$USE_DOCKER" ] && [ "$USE_DOCKER" -ne "0" ]; then
        USE_DOCKER=1
    else
        USE_DOCKER=0
    fi

    if [ -n "$TEST_MODE" ] && [ "$TEST_MODE" -ne "0" ]; then
        TEST_MODE=1
    else
        TEST_MODE=0
    fi

    # set global settlement engine dir so that the engine never gets compiled many times when test
    # furthermore, we cannot clone `settlement-engine` in `interledger-rs` directory.
    if [ -z "${SETTLEMENT_ENGINE_INSTALLL_DIR}" ]; then
        SETTLEMENT_ENGINE_INSTALLL_DIR=$(cd ~; pwd)
    fi

    # define commands
    CMD_DOCKER=docker
    if [ -n "$USE_SUDO" ] && [ "$USE_SUDO" -ne "0" ]; then
        CMD_DOCKER="sudo $CMD_DOCKER"
    fi
}

# run hook_before_kill function only if it exists
function run_hook_before_kill {
    # only when the hook is defined
    type hook_before_kill &>/dev/null
    if [ $? -eq 0 ]; then
        hook_before_kill
    fi
}

# $1 = error message
#
# error_and_exit "Error! Try again."
function error_and_exit() {
    printf "\e[31m%b\e[m\n" "$1" 1>&2
    exit 1
}

# $1 = url
# $2 = timeout, default: -1 = don't timeout
# returns 0 if succeeds
# returns 1 if timeouts
#
# wait_to_serve "http://localhost:7770" 10
function wait_to_serve() {
    local timeout=${2:--1}
    local start=$SECONDS
    while :
    do
        printf "."
        sleep 1
        curl $1 &> /dev/null
        if [ $? -eq 0 ]; then
            break
        fi
        if [ $timeout -ge 0 ] && [ $(($SECONDS - $start)) -ge $timeout ]; then
          return 1
        fi
    done
    return 0
}

# $1 = expected body
# $2 = timeout, -1 = don't timeout
# $3.. = curl arguments (excludes curl itself)
#
# wait_to_get '{"balance":"0"}' -1 -H "Authorization: Bearer xxx" "http://localhost/"
function wait_to_get_http_response_body() {
    local expected=$1
    local timeout=$2
    local start=$SECONDS
    shift
    while :
    do
        printf "."
        sleep 1
        local json=$(curl "$@" 2> /dev/null)
        if [ "$json" = "$expected" ]; then
            break
        fi
        if [ $timeout -ge 0 ] && [ $(($SECONDS - $start)) -ge $timeout ]; then
          return 1
        fi
    done
    return 0
}

# $1 = prompt text
# $2 = default value in [yn]
# sets PROMPT_ANSWER in [yn]
#
# prompt_yn "Quit? [y/N]" n
function prompt_yn() {
    if [ $# -ne 2 ]; then
        return 1
    fi
    local text=$1
    local default=$2
    if ! [[ $default =~ [yn] ]]; then
        return 2
    fi
    read -p "$text" -n 1 answer
    if [[ "$answer" =~ ^[^yYnN]+ ]]; then
        printf "\n"
        prompt_yn "$text" "$default"
        return $?
    fi
    case "$answer" in
        [Yy]) PROMPT_ANSWER=y;;
        [Nn]) PROMPT_ANSWER=n;;
        *) PROMPT_ANSWER=$default;;
    esac
}

# $1.. = curl arguments (excludes curl itself)
# sets TEST_RESULT to http response body
#
# test_http_response_body -H "Authorization: Bearer xxx" "http://localhost/"
function test_http_response_body() {
    TEST_RESULT=$(curl "$@" 2> /dev/null)
    return $?
}

# $1 expected value
# $2.. test function and its arguments
#
# test_equals_or_exit '{"value":true}' test_http_response_body -H "Authorization: Bearer xxx" "http://localhost/"
function test_equals_or_exit() {
    local expected_value="$1"
    shift
    "$@"
    if [ "$TEST_RESULT" = "$expected_value" ]; then
        return 0
    else
        error_and_exit "Test failed. Expected: $expected_value, Got: $TEST_RESULT"
    fi
}
