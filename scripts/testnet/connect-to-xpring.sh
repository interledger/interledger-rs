#!/usr/bin/env bash

# $1 = error message
#
# error_and_exit "Error! Try again."
error_and_exit() {
    colored_output 31 1 "${1}\n" 1>&2
    exit 1
}

# 31 = Red
# 32 = Green
# 33 = Yellow
colored_output() {
    local color="${1:-31}"
    local style="${2:-1}"
    local message="${3:-}"
    printf "\e[%b%bm%b\e[m" "${color}" ";${style}" "${message}"
}

check_command() {
    local command=$1
    local install_name=$2

    printf "Checking if you have ${command}..."
    which ${command}
    if [ $? -eq 0 ]; then
        return 0
    else
        error_and_exit "\nNot found. You have to install ${install_name} first."
    fi
}

# $1 = url
# $2 = timeout, default: -1 = don't timeout
# returns 0 if succeeds
# returns 1 if timeouts
#
# wait_to_serve "http://localhost:7770" 10
wait_to_serve() {
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

usage() {
    cat << EOM
USAGE:
    $(basename $0)

    e.g.
    $(basename $0) -s -c
    $(basename $0) -i

    Compatible with rs3 node with ilp-node-v0.4.1-beta.2

FLAGS:
    -c  Clear config and DB cache.
    -s  Spin up settlement engine.
    -i  Show instructions (Only available after spinning up the node)
    -h  Show this help.

SEE:
    https://xpring.io/ilp-testnet-creds
    https://status.xpring.tech/
EOM
}

prompt_with_message() {
    local message=$1
    local variable_name=$2

    read -p "$message" answer
    if [ -z "$answer" ]; then
        printf "\n"
        prompt_with_message "$message" "$variable_name"
        return $?
    fi
    eval "${variable_name}=\"${answer}\""
}

configure() {
    # if the config already exists, just use it
    if [ -e "${CONFIG_NAME}" ]; then
        ADMIN_AUTH_TOKEN=$(cat "${CONFIG_NAME}" | jq -r ".admin_auth_token")
        printf "Using admin auth from config: %s\n" "${ADMIN_AUTH_TOKEN}"
        return 0
    fi

#    prompt_with_message "ILP address of your node: " "ilp_address"
#    if [[ ! "$ilp_address" =~ ^test\. ]]; then
#        error_and_exit "Specify correct ILP address. It should start with 'test.' (your option: ${ilp_address})."
#    fi
#    # https://github.com/interledger/rfcs/blob/master/0015-ilp-addresses/0015-ilp-addresses.md
#    if [[ ! "$ilp_address" =~ ^(g|private|example|peer|self|test[1-3]?|local)([.][a-zA-Z0-9_~-]+)+$ ]]; then
#        error_and_exit "Specify correct ILP address. (your option: ${ilp_address})."
#    fi

    prompt_with_message "Your node's admin auth token: " "ADMIN_AUTH_TOKEN"

    # set variables
    secret_seed=$(openssl rand -hex 32) || error_and_exit "Could not generate secret_seed."

    # export config.json
    # Currently ilp_address is not used as we are going to spin up a node as `Child` mode.
    cat "${CONFIG_TEMPLATE_FILE}" | sed \
        -e "s/<secret_seed>/${secret_seed}/g" \
        -e "s/<admin_auth_token>/${ADMIN_AUTH_TOKEN}/g" \
        > ${CONFIG_NAME} || error_and_exit "Error exporting config.json"

    colored_output 32 1 "Successfuly configured your '${CONFIG_NAME}'.\n"
}

free_ports() {
    # redis
    for port in $(seq 6379 6380); do
        if lsof -Pi :6379 -sTCP:LISTEN -t >/dev/null ; then
            redis-cli -p ${port} shutdown
        fi
    done

    # killing just in case
    for port in $(seq 6379 6380); do
        if lsof -tPi :${port} >/dev/null ; then
            kill `lsof -tPi :${port}` 2>/dev/null
        fi
    done

    # node
    if lsof -tPi :7770 >/dev/null ; then
        kill `lsof -tPi :7770` 2>/dev/null
    fi

    # se
    if lsof -tPi :3000 >/dev/null ; then
        kill `lsof -tPi :3000` 2>/dev/null
    fi
}

determine_asset_code() {
    # if the asset code is already defined
    if [[ "${ASSET_CODE}" =~ (xrp|eth) ]]; then
        return 0
    fi

    # ask user
    prompt_with_message "Asset code (xrp|eth): " "ASSET_CODE"
    if [[ ! "${ASSET_CODE}" =~ (xrp|eth) ]]; then
        error_and_exit "Specify xrp or eth for the connection type (your option: ${ASSET_CODE})."
    fi
}

spin_up_settlement_engine() {
    # if the credential does not exist
    if [ -e "${CREDENTIAL_NAME}" ]; then
        # should be lowercase here
        ASSET_CODE=$(cat "${CREDENTIAL_NAME}" | jq -r ".asset_code" | tr '[A-Z]' '[a-z]')
    else
        determine_asset_code
    fi

    printf "Spinning up SE Redis..."
    redis-server --port 6380 &> logs/redis_se.log &
    sleep 1
    colored_output 32 1 "done\n"

    if [ "${ASSET_CODE}" = "xrp" ]; then
        printf "Spinning up XRP settlement engine..."
        DEBUG="settlement* ilp-settlement-xrp" \
        CONNECTOR_URL="http://localhost:7771" \
        REDIS_PORT=6380 \
        ENGINE_PORT=3000 \
        ilp-settlement-xrp \
        &> logs/settlement-engine-xrpl.log &
        SETTLEMENT_ENGINE_URL="http://localhost:3000"
        colored_output 32 1 "done\n"
    elif [ "${ASSET_CODE}" = "eth" ]; then
        printf "Compiling code\n"
        cargo build --all-features --bin ilp-node --bin interledger-settlement-engines
        colored_output 32 1 "done\n"
        printf "Spinning up Ethereum settlement engine..."

        cat << "EOM"


You'll need to have an Ethereum endpoint URL.
If you don't have any, try to use Infura. https://infura.io/
The Xpring testnet uses Rinkeby, so you need to connect to the Rinkeby testnet.

EOM
        prompt_with_message "Input Ethereum endpoint URL: " "SE_ETH_URL"
        if [[ ! "${SE_ETH_URL}" =~ http[s]*:\/\/ ]]; then
            # TODO check
            error_and_exit "Specify correct URL (your option: ${SE_ETH_URL})."
        fi

        # specify chain_id=4 for Rinkeby
        cargo run --all-features --bin interledger-settlement-engines -- ethereum-ledger \
        --private_key "${SE_ETH_SECRET}" \
        --chain_id 4 \
        --confirmations 0 \
        --poll_frequency 10000 \
        --ethereum_url "${SE_ETH_URL}" \
        --connector_url http://127.0.0.1:7771 \
        --redis_url redis://127.0.0.1:6380/ \
        --settlement_api_bind_address 127.0.0.1:3000 \
        &> logs/settlement-engine-eth.log &

        SETTLEMENT_ENGINE_URL="http://localhost:3000"
        colored_output 32 1 "done\n"
    else
        error_and_exit "Invalid asset code! ${ASSET_CODE}"
    fi
}

stop_localtunnels() {
    if [ -e "${NODE_LT_PID_FILE}" ]; then
        kill $(<"${NODE_LT_PID_FILE}") 2>/dev/null
        rm "${NODE_LT_PID_FILE}"
    fi
}

clear_redis() {
    for port in $(seq 6379 6380); do
        if lsof -Pi :6379 -sTCP:LISTEN -t >/dev/null ; then
            redis-cli -p ${port} flushall
        fi
    done
}

spin_up_node() {
    printf "Compiling code\n"
    cargo build --all-features --bin ilp-node --bin interledger-settlement-engines
    colored_output 32 1 "done\n"

    printf "Spinning up Redis..."
    redis-server --port 6379 &> logs/redis.log &
    sleep 1
    colored_output 32 1 "done\n"

    printf "Spinning up a node"
    cargo run --all-features --bin ilp-node -- config.json &> logs/node.log &
    wait_to_serve http://localhost:7770/ 10 || error_and_exit "Error spinning up a node.\nCheck log files."
    colored_output 32 1 "done\n"

    colored_output 33 21 "Logs are written in logs directory.\n"
}

set_up_localtunnel() {
    printf "Setting up localtunnels..."
    if [ -e "${NODE_LT_SUBDOMAIN_FILE}" ]; then
        NODE_LT_SUBDOMAIN=$(cat "${NODE_LT_SUBDOMAIN_FILE}")
    else
        # TODO more readable name?
        local sub_domain_base=$(openssl rand -hex 8)
        # localtunnel doesn't accept subdomains which contain dot
        NODE_LT_SUBDOMAIN="node-${sub_domain_base}"
        echo "${NODE_LT_SUBDOMAIN}" > "${NODE_LT_SUBDOMAIN_FILE}"
    fi

    lt -p 7770 -s "${NODE_LT_SUBDOMAIN}" &>logs/localtunnel.log &
    printf "$!" > ${NODE_LT_PID_FILE}
    colored_output 32 1 "done\n"
    colored_output 33 21 "Node URL: $(get_localtunnel_url ${NODE_LT_SUBDOMAIN})\n"
}

get_localtunnel_url() {
    printf "https://%s.localtunnel.me" "${1}"
}

stop_services() {
    printf "Shutting down services..."
    free_ports
    stop_localtunnels
    colored_output 32 1 "done\n"
}

inject_json_into_variable() {
    local json=$1
    local prefix=$2
    local keys=$(echo "${json}" | jq -r ".|keys|.[]")

    # WARN potentially not safe, evaling values from the server.
    for key in $keys; do
        local value=$(echo "${json}" | jq -r ".${key}")
        eval "${prefix}_${key}=${value}"
    done
}

add_accounts() {
    # if the credential does not exist
    if [ ! -e "${CREDENTIAL_NAME}" ]; then
        determine_asset_code
        printf "Retrieving credential information..."
        curl ${XPRING_CREDENTIAL_API}/accounts/${ASSET_CODE} 2>/dev/null > "${CREDENTIAL_NAME}" || error_and_exit "Could not retrieve credential information."
        colored_output 32 1 "done\n"
    fi

    local credential=$(cat "${CREDENTIAL_NAME}")
    inject_json_into_variable "${credential}" "credential"

    # load settings from setting file if needed
    if [ -z "${ADMIN_AUTH_TOKEN}" ]; then
        if [ -e "${CONFIG_NAME}" ]; then
            ADMIN_AUTH_TOKEN=$(cat ${CONFIG_NAME} | jq -r ".admin_auth_token")
            colored_output 33 21 "Admin auth token is loaded from the config file.\n"
        else
            prompt_with_message "Your node's admin auth token: " "admin_auth_token"
        fi
    fi

    # generate xpring secret
    local xpring_secret=$(openssl rand -hex 32) || error_and_exit "Could not generate secret."
    colored_output 33 21 "Auto generated Xpring incoming token: ${xpring_secret}\n"

    # create Xpring account json
    # TODO: ilp_over_btp_url should be fixed.
    local xpring_account_json=$(cat "${ACCOUNT_TEMPLATE_FILE}" | sed \
        -e "s/<ilp_address>/${XPRING_NODE_ILP_ADDRESS}/g" \
        -e "s/<username>/${XPRING_USERNAME}/g" \
        -e "s/<asset_code>/${credential_asset_code}/g" \
        -e "s/<asset_scale>/${credential_asset_scale}/g" \
        -e "s~<ilp_over_btp_url>~btp\+wss://${credential_node}/ilp/btp~g" \
        -e "s/<ilp_over_btp_incoming_token>/${xpring_secret}/g" \
        -e "s/<ilp_over_btp_outgoing_token>/${credential_username}:${credential_passkey}/g" \
        -e "s~<ilp_over_http_url>~https://${credential_node}/ilp~g" \
        -e "s/<ilp_over_http_incoming_token>/${xpring_secret}/g" \
        -e "s/<ilp_over_http_outgoing_token>/${credential_username}:${credential_passkey}/g" \
        -e "s/<routing_relation>/Parent/g")

    # TODO: could be more efficient
    echo ${xpring_account_json} | jq "." > logs/accounts/xpring_account.json
    if [ -n "${SETTLEMENT_ENGINE_URL}" ]; then
        # only if the settlement engine is defined
        xpring_account_json=$(cat "logs/accounts/xpring_account.json" | sed -e "s~<settlement_engine_url>~${SETTLEMENT_ENGINE_URL}~g")
    else
        # remove if settlement engine url is not defined
        xpring_account_json=$(cat "logs/accounts/xpring_account.json" | sed \
            -e "/settle_to/d" \
            -e "/settle_threshold/d" \
            -e "/settlement_engine_url/d")
    fi

    # insert the Xpring account into our node
    printf "Inserting the Xpring account into our node..."
    echo ${xpring_account_json} | jq "." > logs/accounts/xpring_account.json
    echo ${xpring_account_json} | curl \
        -X POST \
        -H "Authorization: Bearer ${ADMIN_AUTH_TOKEN}" \
        -H "Content-Type: application/json" \
        -d @- \
        http://localhost:7770/accounts >logs/xpring_account.log 2>/dev/null || error_and_exit "Could not insert the Xpring account."
    colored_output 32 1 "done\n"

    # create our account json
    # TODO: ilp_over_btp_url should be fixed.
    local our_account_json=$(cat "${ACCOUNT_TEMPLATE_FILE}" | sed \
        -e "/ilp_over_btp/d" \
        -e "/ilp_over_http_url/d" \
        -e "/ilp_over_http_outgoing_token/d" \
        -e "/settlement_engine_url/d" \
        -e "/settle_threshold/d" \
        -e "/settle_to/d" \
        -e "s/<ilp_address>/${XPRING_NODE_ILP_ADDRESS}.${credential_username}/g" \
        -e "s/<username>/${credential_username}/g" \
        -e "s/<asset_code>/${credential_asset_code}/g" \
        -e "s/<asset_scale>/${credential_asset_scale}/g" \
        -e "s/<ilp_over_http_incoming_token>/${credential_passkey}/g" \
        -e "s/<routing_relation>/NonRoutingAccount/g")

    # insert our account into our node
    printf "Inserting our account into our node..."
    echo ${our_account_json} | jq "." > logs/accounts/our_account.json
    echo ${our_account_json} | curl \
        -X POST \
        -H "Authorization: Bearer ${ADMIN_AUTH_TOKEN}" \
        -H "Content-Type: application/json" \
        -d @- \
        http://localhost:7770/accounts >logs/our_account.log 2>/dev/null || error_and_exit "Could not insert our account."
    colored_output 32 1 "done\n"

    # download our account setting
    curl \
        -X GET \
        -H "Authorization: Bearer ${credential_username}:${credential_passkey}" \
        https://${credential_node}/accounts/${credential_username} >logs/accounts/our_delegate_account.json 2>/dev/null || error_and_exit "Could not download our account."

    # create our delegate account json
    local our_ilp_over_http_url="$(get_localtunnel_url ${NODE_LT_SUBDOMAIN})/ilp"
    local our_ilp_over_btp_url="$(get_localtunnel_url ${NODE_LT_SUBDOMAIN})/ilp/btp"
    our_ilp_over_btp_url="${our_ilp_over_btp_url//https/btp+wss}"
    local replace_jq=".ilp_over_http_url |= \"${our_ilp_over_http_url}\" |\
        .ilp_over_http_incoming_token |= \"${credential_passkey}\" |\
        .ilp_over_http_outgoing_token |= \"xpring:${xpring_secret}\" |\
        .ilp_over_btp_url |= \"${our_ilp_over_btp_url}\" |\
        .ilp_over_btp_incoming_token |= \"${credential_passkey}\" |\
        .ilp_over_btp_outgoing_token |= \"xpring:${xpring_secret}\""
    local our_delegate_account_json=$(cat "logs/accounts/our_delegate_account.json" | jq "${replace_jq}")

    # update our delegate account on Xpring's node so that they know our ILP over HTTP URL
    printf "Updating our account on Xpring's node..."
    echo ${our_delegate_account_json} | jq "." > logs/accounts/our_delegate_account.json
    echo ${our_delegate_account_json} | curl \
        -X PUT \
        -H "Authorization: Bearer ${credential_username}:${credential_passkey}" \
        -H "Content-Type: application/json" \
        -d @- \
        https://${credential_node}/accounts/${credential_username}/settings >logs/our_delegate_account.log 2>/dev/null || error_and_exit "Could not update our account."
    colored_output 32 1 "done\n"

    # output accounts
    # local accounts
    printf "Your local accounts:\n"
    curl \
        -X GET \
        -H "Authorization: Bearer ${ADMIN_AUTH_TOKEN}" \
        http://localhost:7770/accounts 2>/dev/null | jq "."

    # remote account
    printf "\nYour remote account:\n"
    curl \
        -X GET \
        -H "Authorization: Bearer ${credential_username}:${credential_passkey}" \
        https://${credential_node}/accounts/${credential_username} 2>/dev/null | jq "."
}

show_instructions() {
    # output instructions
    local spsp_url="$(get_localtunnel_url ${NODE_LT_SUBDOMAIN})/accounts/${credential_username}/spsp"
    local instructions="You can send payments as follows:

    curl \\
        -H \"Authorization: Bearer ${credential_username}:${credential_passkey}\" \\
        -H \"Content-Type: application/json\" \\
        -d '{\"receiver\":\"RECEIVER SPSP ADDRESS\",\"source_amount\":AMOUNT}' \\
        http://localhost:7770/accounts/${credential_username}/payments

You can pass the following URL as your SPSP URL:

    ${spsp_url}

You can also check your balance as follows:

    curl \\
        -H \"Authorization: Bearer ${credential_username}:${credential_passkey}\" \\
        http://localhost:7770/accounts/${credential_username}/balance"

    echo "${instructions}" > ${INSTRUCTION_FILE}
    printf "\n%s\n\n" "${instructions}"
}

# show usage if needed
if [ $# -eq 0 ]; then
    usage
    exit
fi

while getopts scih: OPT
do
    case $OPT in
        s) CMD_SPIN_UP_SE=1 ;;
        c) CLEAR_CACHE=1 ;;
        i) SHOW_INSTRUCTIONS=1 ;;
        h)  usage; exit 0 ;;
        \?) usage; exit 0 ;;
    esac
done
shift $(($OPTIND - 1))

mkdir -p logs/accounts

# set up global variables
BASE_DIR=$(cd $(dirname $0); pwd)
XPRING_USERNAME="xpring"
XPRING_CREDENTIAL_API="https://xpring.io/api"
# XPRING_CREDENTIAL_API="https://stage.xpring.io/api"
XPRING_NODE_ILP_ADDRESS="test.xpring-dev.rs3"
XPRING_NODE_API_URL="https://rs3.xpring.dev"
NODE_LT_SUBDOMAIN_FILE="node_lt_subdomain.config"
INSTRUCTION_FILE="logs/instructions.txt"
CREDENTIAL_NAME="credential.json"
CONFIG_NAME="config.json"
CONFIG_TEMPLATE_NAME="config-template.json"
CONFIG_TEMPLATE_FILE="${BASE_DIR}/${CONFIG_TEMPLATE_NAME}"
ACCOUNT_TEMPLATE_NAME="account-template.json"
ACCOUNT_TEMPLATE_FILE="${BASE_DIR}/${ACCOUNT_TEMPLATE_NAME}"
NODE_LT_PID_FILE="logs/localtunnel.pid"
# because this is for the testnet, OK to commit. Normally it should not be.
SE_ETH_SECRET="380EB0F3D505F087E438ECA80BC4DF9A7FAA24F868E69FC0440261A0FC0567DC"
export RUST_LOG=interledger=trace

# show instructions should run first
if [ "${SHOW_INSTRUCTIONS}" = "1" ]; then
    if [ -e "${INSTRUCTION_FILE}" ]; then
        cat logs/instructions.txt
        exit 0
    else
        error_and_exit "You have to configure node first!"
    fi
fi

# check commands
check_command "lt" "localtunnel"
check_command "jq" "jq"
check_command "openssl" "openssl"
check_command "redis-server" "redis"
check_command "cargo" "Rust"

if [ "${CLEAR_CACHE}" = "1" ]; then
    if [ -e "${CONFIG_NAME}" ]; then
        rm "${CONFIG_NAME}"
    fi
    if [ -e "${CREDENTIAL_NAME}" ]; then
        rm "${CREDENTIAL_NAME}"
    fi
    if [ -e "${NODE_LT_SUBDOMAIN_FILE}" ]; then
        rm "${NODE_LT_SUBDOMAIN_FILE}"
    fi
    clear_redis
fi

stop_services
configure

if [ "${CMD_SPIN_UP_SE}" = "1" ]; then
    check_command "ilp-settlement-xrp" "settlement-xrp"
    spin_up_settlement_engine
fi

spin_up_node
set_up_localtunnel
add_accounts
show_instructions
