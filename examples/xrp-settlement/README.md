<!--!
# For integration tests
function post_test_hook() {
    if [ $TEST_MODE -eq 1 ]; then
        test_equals_or_exit '{"asset_code":"XRP","balance":-0.0005}' test_http_response_body -H "Authorization: Bearer in_alice" http://localhost:7770/accounts/alice/balance
        test_equals_or_exit '{"asset_code":"XRP","balance":0.0}' test_http_response_body -H "Authorization: Bearer bob_password" http://localhost:7770/accounts/bob/balance
        test_equals_or_exit '{"asset_code":"XRP","balance":0.0}' test_http_response_body -H "Authorization: Bearer alice_password" http://localhost:8770/accounts/alice/balance
        test_equals_or_exit '{"asset_code":"XRP","balance":0.0005}' test_http_response_body -H "Authorization: Bearer in_bob" http://localhost:8770/accounts/bob/balance
    fi
}
-->

# Interledger with XRP On-Ledger Settlement

> A demo that sends payments between 2 Interledger.rs nodes and settles using XRP transactions.

## Overview

This example shows how to configure Interledger.rs nodes and use the XRP testnet as a settlement ledger for payments sent between the nodes. You'll find many useful resources about the XRP Ledger (XRPL) [here](https://xrpl.org). To learn about settlement in Interledger, refer to [Peering, Clearing and Settling](https://github.com/interledger/rfcs/blob/master/0032-peering-clearing-settlement/0032-peering-clearing-settlement.md).

To run the full example, you can use [`run-md.sh`](../../scripts/run-md.sh) as described [here](../README.md). Otherwise, you can walk through each step below.

Each of the services write their logs to files found under the `logs` directory. You can run `tail -f logs/node_a.log`, for example, to watch the logs of Node A.

![overview](images/overview.svg)

## Prerequisites

- [Rust](#rust)
- [XRP Settlement Engine](#xrp-settlement-engine)
- [Redis](#redis)

### Rust

Because Interledger.rs is written in the Rust language, you need the Rust environment. Refer to the [Getting started](https://www.rust-lang.org/learn/get-started) page or just `curl https://sh.rustup.rs -sSf | sh` and follow the instructions.

### XRP Settlement Engine

Interledger.rs and settlement engines written in other languages are fully interoperable. Here, we'll use the [XRP Ledger Settlement Engine](https://github.com/interledgerjs/settlement-xrp/), which is written in TypeScript. We'll need `node` and `npm` to install and run the settlement engine. If you don't have it already, refer to [Install Node.js](#install-nodejs).

Install the settlement engine as follows:

```bash #
npm i -g ilp-settlement-xrp
```

(This makes the `ilp-settlement-xrp` command available to your PATH.)

#### Install Node.js

(In case you don't have Node.js) There are a few ways to install Node.js. If you work on multiple JavaScript or TypeScript projects which require different `node` versions, using `nvm` may be suitable.

- [`nvm`](https://github.com/nvm-sh/nvm) (node version manager)
  - macOS: If you use [Homebrew](https://brew.sh/), run `brew install nvm` and you'll see some additional instructions. Follow it and `nvm install node` and `nvm use node`.
  - others: Refer to [`nvm`](https://github.com/nvm-sh/nvm) site.
- Install independently
  - macOS: If you use [Homebrew](https://brew.sh/), run `brew install node`
  - Ubuntu: `sudo apt-get install nodejs npm`

Then you should be able to use `npm`.

### Redis

The Interledger.rs nodes and settlement engines currently use [Redis](https://redis.io/) to store their data (SQL database support coming soon!). Nodes and settlement engines can use different Redis instances.

- Compile and install from the source code
  - [Download the source code here](https://redis.io/download)
- Install using package managers
  - Ubuntu: run `sudo apt-get install redis-server`
  - macOS: If you use Homebrew, run `brew install redis`

Make sure your Redis is empty. You could run `redis-cli flushall` to clear all the data.

## Instructions

<!--!
# import some functions from run-md-lib.sh
# this variable is set by run-md.sh
source $RUN_MD_LIB
init

printf "Stopping Interledger nodes...\n"

for port in `seq 6379 6382`; do
    if lsof -Pi :${port} -sTCP:LISTEN -t ; then
        redis-cli -p ${port} shutdown
    fi
done

if [ -f dump.rdb ] ; then
    rm -f dump.rdb
fi

for port in 8545 7770 8770 3000 3001; do
    if lsof -tPi :${port} ; then
        kill `lsof -tPi :${port}`
    fi
done
-->

### 1. Prepare interledger.rs

First of all, we have to prepare `interledger.rs`. You can either:

1. Download compiled binaries
1. Compile from the source code

Compiling the source code is relatively slow, so we recommend downloading the pre-built binaries unless you want to modify some part of the code.

#### Download Compiled Binaries

We provide compiled binaries for

- Linux based OSs
- macOS

First, let's make a directory to install binaries.

```bash
mkdir -p ~/.interledger/bin

# Also write this line in .bash_profile etc if needed
export PATH=~/.interledger/bin:$PATH
```

##### Linux based OSs

<!--!
if [ ${SOURCE_MODE} -ne 1 ]; then
    if [ $(is_linux) -eq 1 ]; then
-->
```bash
pushd ~/.interledger/bin &>/dev/null

# install ilp-node
if [ ! -e "ilp-node" ]; then
    curl -L https://github.com/interledger-rs/interledger-rs/releases/download/ilp-node-latest/ilp-node-x86_64-unknown-linux-musl.tar.gz | tar xzv
fi

# install ilp-cli
if [ ! -e "ilp-cli" ]; then
    curl -L https://github.com/interledger-rs/interledger-rs/releases/download/ilp-cli-latest/ilp-cli-x86_64-unknown-linux-musl.tar.gz | tar xzv
fi

popd &>/dev/null
```
<!--!
    fi
-->

##### macOS

<!--!
    if [ $(is_macos) -eq 1 ]; then
-->
```bash
pushd ~/.interledger/bin &>/dev/null

# install ilp-node
if [ ! -e "ilp-node" ]; then
    curl -L https://github.com/interledger-rs/interledger-rs/releases/download/ilp-node-latest/ilp-node-x86_64-apple-darwin.tar.gz | tar xzv -
fi

# install ilp-cli
if [ ! -e "ilp-cli" ]; then
    curl -L https://github.com/interledger-rs/interledger-rs/releases/download/ilp-cli-latest/ilp-cli-x86_64-apple-darwin.tar.gz | tar xzv -
fi

popd &>/dev/null
```
<!--!
    fi
fi
-->

#### Compile from the Source Code
If you would prefer compiling from the source code, compile interledger.rs and CLI as follows.

<!--!
if [ ${SOURCE_MODE} -eq 1 ]; then
    printf "Building interledger.rs... (This may take a couple of minutes)\n"
-->
```bash
# These aliases make our command invocations more natural
# Be aware that we are using `--` to differentiate arguments for `cargo` from `ilp-node` or `ilp-cli`.
# Arguments before `--` are used for `cargo`, after are used for `ilp-node`.

alias ilp-node="cargo run --quiet --bin ilp-node --"
alias ilp-cli="cargo run --quiet --bin ilp-cli --"

cargo build --bin ilp-node --bin ilp-cli
```
<!--!
fi
-->

### 2. Launch Redis

<!--!
printf "\nStarting Redis instances..."
redis-server --version > /dev/null || error_and_exit "Uh oh! You need to install redis-server before running this example"
-->

```bash
# Create the logs directory if it doesn't already exist
mkdir -p logs

# Start Redis
redis-server --port 6379 &> logs/redis-a-node.log &
redis-server --port 6380 &> logs/redis-a-se.log &
redis-server --port 6381 &> logs/redis-b-node.log &
redis-server --port 6382 &> logs/redis-b-se.log &
```

To remove all the data in Redis, you might additionally perform:

<!--!
sleep 2
printf "done\n"
-->
```bash
for port in `seq 6379 6382`; do
    redis-cli -p $port flushall
done
```

When you want to watch logs, use the `tail` command. You can use the command like: `tail -f logs/redis-a-node.log`

### 3. Launch Settlement Engines

Because each node needs its own settlement engine, we need to launch both a settlement engine for Alice's node and another settlement engine for Bob's node.

By default, the XRP settlement engine generates new testnet XRPL accounts prefunded with 1,000 testnet XRP (a new account is generated each run). Alternatively, you may supply an `XRP_SECRET` environment variable by generating your own testnet credentials from the [official faucet](https://xrpl.org/xrp-test-net-faucet.html).

<!--!
printf "\nStarting settlement engines...\n"
-->

```bash
# Start Alice's settlement engine
DEBUG="settlement*" \
REDIS_URI=127.0.0.1:6380 \
ilp-settlement-xrp \
&> logs/node-alice-settlement-engine.log &

# Start Bob's settlement engine
DEBUG="settlement*" \
CONNECTOR_URL="http://localhost:8771" \
REDIS_URI=127.0.0.1:6382 \
ENGINE_PORT=3001 \
ilp-settlement-xrp \
&> logs/node-bob-settlement-engine.log &
```

### 4. Launch 2 Nodes

<!--!
printf "\n\nStarting Interledger nodes...\n"
-->

```bash
# Turn on debug logging for all of the interledger.rs components
export RUST_LOG=interledger=debug
mkdir -p logs

# Start both nodes.
# Note that the configuration options can be passed as environment variables
# or saved to a YAML, JSON or TOML file and passed to the node as a positional argument.
# You can also pass it from STDIN.

# Start Alice's node
ilp-node \
--ilp_address example.alice \
--secret_seed 8852500887504328225458511465394229327394647958135038836332350604 \
--admin_auth_token hi_alice \
--redis_url redis://127.0.0.1:6379/ \
--http_bind_address 127.0.0.1:7770 \
--settlement_api_bind_address 127.0.0.1:7771 \
&> logs/node-alice.log &

# Start Bob's node
ilp-node \
--ilp_address example.bob \
--secret_seed 1604966725982139900555208458637022875563691455429373719368053354 \
--admin_auth_token hi_bob \
--redis_url redis://127.0.0.1:6381/ \
--http_bind_address 127.0.0.1:8770 \
--settlement_api_bind_address 127.0.0.1:8771 \
&> logs/node-bob.log &
```

<!--!
printf "\nWaiting for Interledger.rs nodes to start up"

wait_to_serve "http://localhost:7770" 10 || error_and_exit "\nFailed to spin up nodes. Check out your configuration and log files."
wait_to_serve "http://localhost:8770" 10 || error_and_exit "\nFailed to spin up nodes. Check out your configuration and log files."
wait_to_serve "http://localhost:3000" 10 || error_and_exit "\nFailed to spin up settlement engine. Check out your configuration and log files."
wait_to_serve "http://localhost:3001" 10 || error_and_exit "\nFailed to spin up settlement engine. Check out your configuration and log files."

printf " done\nThe Interledger.rs nodes are up and running!\n\n"
-->

### 5. Configure the Nodes

<!--!
printf "Creating accounts:\n"
-->

```bash
export ILP_CLI_API_AUTH=hi_alice

printf "Adding Alice's account...\n"
ilp-cli accounts create alice \
    --ilp-address example.alice \
    --asset-code XRP \
    --asset-scale 6 \
    --max-packet-amount 100 \
    --ilp-over-http-incoming-token in_alice \
    --settle-to 0 &> logs/account-alice-alice.log

printf "Adding Bob's Account...\n"
ilp-cli --node http://localhost:8770 accounts create bob \
    --auth hi_bob \
    --ilp-address example.bob \
    --asset-code XRP \
    --asset-scale 6 \
    --max-packet-amount 100 \
    --ilp-over-http-incoming-token in_bob \
    --settle-to 0 &> logs/account-bob-bob.log

printf "Adding Bob's account on Alice's node...\n"
ilp-cli accounts create bob \
    --ilp-address example.bob \
    --asset-code XRP \
    --asset-scale 6 \
    --max-packet-amount 100 \
    --settlement-engine-url http://localhost:3000 \
    --ilp-over-http-incoming-token bob_password \
    --ilp-over-http-outgoing-token alice_password \
    --ilp-over-http-url http://localhost:8770/accounts/alice/ilp \
    --settle-threshold 500 \
    --min-balance -1000 \
    --settle-to 0 \
    --routing-relation Peer &> logs/account-alice-bob.log &

printf "Adding Alice's account on Bob's node...\n"
ilp-cli --node http://localhost:8770 accounts create alice \
    --auth hi_bob \
    --ilp-address example.alice \
    --asset-code XRP \
    --asset-scale 6 \
    --max-packet-amount 100 \
    --settlement-engine-url http://localhost:3001 \
    --ilp-over-http-incoming-token alice_password \
    --ilp-over-http-outgoing-token bob_password \
    --ilp-over-http-url http://localhost:7770/accounts/bob/ilp \
    --settle-threshold 500 \
    --min-balance -1000 \
    --settle-to 0 \
    --routing-relation Peer &> logs/account-bob-alice.log &

sleep 2
```

Now two nodes and its settlement engines are set and accounts for each node are also set up.

Notice how we use Alice's settlement engine endpoint while registering Bob. This means that whenever Alice interacts with Bob's account, she'll use that Settlement Engine.

The `settle_threshold` and `settle_to` parameters control when settlements are triggered. The node will send a settlement when an account's balance reaches the `settle_threshold`, and it will settle for `balance - settle_to`.

### 6. Sending a Payment

<!--!
printf "\n\nChecking balances prior to payment...\n"

printf "\nAlice's balance on Alice's node: "
ilp-cli accounts balance alice

printf "Bob's balance on Alice's node: "
ilp-cli accounts balance bob

printf "Alice's balance on Bob's node: "
ilp-cli --node http://localhost:8770 accounts balance alice --auth hi_bob 

printf "Bob's balance on Bob's node: "
ilp-cli --node http://localhost:8770 accounts balance bob --auth hi_bob 

printf "\n\n"
-->

The following script sends a payment from Alice to Bob.

<!--!
printf "Sending payment of 500 from Alice to Bob\n"
-->

```bash
ilp-cli pay alice \
    --auth in_alice \
    --amount 500 \
    --to http://localhost:8770/accounts/bob/spsp
```

<!--!
printf "\n"

# wait untill the settlement is done
printf "\nWaiting for XRP ledger to be validated"
wait_to_get_http_response_body '{"asset_code":"XRP","balance":0.0}' 20 -H "Authorization: Bearer alice_password" "http://localhost:8770/accounts/alice/balance" || error_and_exit "Could not confirm settlement."
printf "done\n"
-->

### 7. Check Balances

<!--!
printf "Checking balances after payment...\n"
-->

```bash
printf "\nAlice's balance on Alice's node: "
ilp-cli accounts balance alice

printf "Bob's balance on Alice's node: "
ilp-cli accounts balance bob

printf "Alice's balance on Bob's node: "
ilp-cli --node http://localhost:8770 accounts balance alice --auth hi_bob 

printf "Bob's balance on Bob's node: "
ilp-cli --node http://localhost:8770 accounts balance bob --auth hi_bob 
```

### 8. Kill All the Services

Finally, you can stop all the services as follows:

```bash #
for port in `seq 6379 6382`; do
    if lsof -Pi :${port} -sTCP:LISTEN -t >/dev/null ; then
        redis-cli -p ${port} shutdown
    fi
done

if [ -f dump.rdb ] ; then
    rm -f dump.rdb
fi

for port in 8545 7770 8770 3000 3001; do
    if lsof -tPi :${port} >/dev/null ; then
        kill `lsof -tPi :${port}`
    fi
done
```

## Advanced

### Check the Incoming Settlement on XRPL

You'll find incoming settlement logs in your settlement engine logs. Try:

```bash #
cat logs/node-bob-settlement-engine.log | grep "Received incoming XRP payment"
```

<!--!
printf "\n\nYou could also try the following command to check if XRPL incoming payment is done.\n\n"
printf "\tcat logs/node-bob-settlement-engine.log | grep \"Received incoming XRP payment\"\n"
printf "\n"
run_post_test_hook
if [ $TEST_MODE -ne 1 ]; then
    prompt_yn "Do you want to kill the services? [Y/n] " "y"
fi
printf "\n"
if [ "$PROMPT_ANSWER" = "y" ] || [ $TEST_MODE -eq 1 ] ; then
    exec 2>/dev/null
    for port in `seq 6379 6382`; do
        if lsof -Pi :${port} -sTCP:LISTEN -t >/dev/null ; then
            redis-cli -p ${port} shutdown
        fi
    done

    if [ -f dump.rdb ] ; then
        rm -f dump.rdb
    fi

    for port in 7770 8770 3000 3001; do
        if lsof -tPi :${port} >/dev/null ; then
            kill `lsof -tPi :${port}`
        fi
    done
fi
-->

## Troubleshooting

```
# When installing Node.js with apt-get
E: Unable to locate package nodejs
E: Unable to locate package npm
```

Try `sudo apt-get update`.

## Conclusion

This example showed an SPSP payment sent between two Interledger.rs nodes that settled using on-ledger XRP transactions.

Check out the [other examples](../README.md) for more complex demos that show other features of Interledger, including multi-hop routing and cross-currency payments.
