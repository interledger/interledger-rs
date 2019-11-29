#!/bin/bash

# initialize global variables
function init() {
    if [ -n "$SOURCE_MODE" ] && [ "$SOURCE_MODE" -ne "0" ]; then
        SOURCE_MODE=1
    else
        SOURCE_MODE=0
    fi

    if [ -n "$TEST_MODE" ] && [ "$TEST_MODE" -ne "0" ]; then
        TEST_MODE=1
    else
        TEST_MODE=0
    fi
}

# run pre_test_hook function only if it exists
function run_pre_test_hook {
    # only when the hook is defined
    type pre_test_hook &>/dev/null
    if [ $? -eq 0 ]; then
        pre_test_hook
    fi
}

# run post_test_hook function only if it exists
function run_post_test_hook {
    # only when the hook is defined
    type post_test_hook &>/dev/null
    if [ $? -eq 0 ]; then
        post_test_hook
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
        curl $1 &> /dev/null
        if [ $? -eq 0 ]; then
            break
        fi
        if [ $timeout -ge 0 ] && [ $(($SECONDS - $start)) -ge $timeout ]; then
          return 1
        fi
        sleep 1
    done
    return 0
}

# $1 = expected body
# $2 = timeout, -1 = don't timeout
# $3.. = curl arguments (excludes curl itself)
#
# wait_to_get '{"balance":0, "asset_code": "ABC"}' -1 -H "Authorization: Bearer xxx" "http://localhost/"
function wait_to_get_http_response_body() {
    local expected=$1
    local timeout=$2
    local start=$SECONDS
    shift
    while :
    do
        printf "."
        local json=$(curl "$@" 2> /dev/null)
        if [ "$json" = "$expected" ]; then
            break
        fi
        if [ $timeout -ge 0 ] && [ $(($SECONDS - $start)) -ge $timeout ]; then
          return 1
        fi
        sleep 1
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

function is_linux() {
    if [[ $(uname) =~ Linux ]]; then
        echo 1
    else
        echo 0
    fi
}

function is_macos() {
    if [[ $(uname) =~ Darwin ]]; then
        echo 1
    else
        echo 0
    fi
}
