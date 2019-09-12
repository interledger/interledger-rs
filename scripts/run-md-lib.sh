#!/bin/bash

# $1 = error message
#
# error_and_exit "Error! Try again."
function error_and_exit() {
    printf "\e[31m$1\e[m\n" 1>&2
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

# $1 = expected content
# $2.. = curl arguments (excludes curl itself)
#
# wait_to_get '{"balance":"0"}' -H "Authorization: Bearer xxx" "http://localhost/"
function wait_to_get() {
    local expected=$1
    shift
    while :
    do
        printf "."
        sleep 1
        local json=$(curl "$@" 2> /dev/null)
        if [ "$json" = "$expected" ]; then
            break
        fi
    done
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
