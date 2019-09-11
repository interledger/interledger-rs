#!/bin/bash

function error_and_exit() {
    printf "\e[31m$1\e[m\n"
    exit 1
}

function wait_to_serve() {
    while :
    do
        printf "."
        sleep 1
        curl $1 &> /dev/null
        if [ $? -eq 0 ]; then
            break
        fi
    done
}

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
