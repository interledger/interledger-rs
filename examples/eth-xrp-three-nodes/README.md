<!--!
# For integration tests
function post_test_hook() {
    if [ $TEST_MODE -eq 1 ]; then
        test_equals_or_exit '{"asset_code":"ETH","balance":-0.0005}' test_http_response_body -H "Authorization: Bearer hi_alice" http://localhost:7770/accounts/alice/balance
        test_equals_or_exit '{"asset_code":"ETH","balance":0.0}' test_http_response_body -H "Authorization: Bearer hi_alice" http://localhost:7770/accounts/bob/balance
        test_equals_or_exit '{"asset_code":"ETH","balance":0.0}' test_http_response_body -H "Authorization: Bearer hi_bob" http://localhost:8770/accounts/alice/balance
        test_equals_or_exit '{"asset_code":"XRP","balance":0.0}' test_http_response_body -H "Authorization: Bearer hi_bob" http://localhost:8770/accounts/charlie/balance
        test_equals_or_exit '{"asset_code":"XRP","balance":0.0}' test_http_response_body -H "Authorization: Bearer hi_charlie" http://localhost:9770/accounts/bob/balance
        test_equals_or_exit '{"asset_code":"XRP","balance":0.0005}' test_http_response_body -H "Authorization: Bearer hi_charlie" http://localhost:9770/accounts/charlie/balance
    fi
}
-->

# Interledger with Ethereum and XRP On-Ledger Settlement

> A demo that sends payments between 3 Interledger.rs nodes and settles using Ethereum transactions and XRP transactions.

## Overview

This example shows how to configure Interledger.rs nodes and use an Ethereum network (testnet or mainnet) and the XRP Ledger testnet as settlement ledgers for payments sent between the nodes. If you are new to Ethereum, you can learn about it [here](https://www.ethereum.org/beginners/). You'll find many useful resources about the XRP Ledger (XRPL) [here](https://xrpl.org). To learn about settlement in Interledger, refer to [Peering, Clearing and Settling](https://github.com/interledger/rfcs/blob/master/0032-peering-clearing-settlement/0032-peering-clearing-settlement.md).

To run the full example, you can use [`run-md.sh`](../../scripts/run-md.sh) as described [here](../README.md). Otherwise, you can walk through each step below.

Each of the services write their logs to files found under the `logs` directory. You can run `tail -f logs/node_a.log`, for example, to watch the logs of Node A.

![overview](images/overview.svg)

## Prerequisites

- [Rust](#rust)
- [An Ethereum network](#an-ethereum-network) to connect to
- [XRP Settlement Engine](#settlement*)
- [Redis](#redis)

### Rust

Because Interledger.rs is written in the Rust language, you need the Rust environment. Refer to the [Getting started](https://www.rust-lang.org/learn/get-started) page or just `curl https://sh.rustup.rs -sSf | sh` and follow the instructions.

### An Ethereum network

First, you need an Ethereum network. You can either use a local testnet, a remote testnet, or the mainnet.

For this example, we'll use [ganache-cli](https://github.com/trufflesuite/ganache-cli) which deploys a local Ethereum testnet at `localhost:8545`. To install `ganache-cli`, run `npm install -g ganache-cli`. If you do not already have Node.js installed on your computer, you can follow [the instructions below](#install-nodejs) to install it.

Advanced: You can run this against the Rinkeby Testnet by running a node that connects to Rinkeby (e.g. `geth --rinkeby --syncmode "light"`) or use a third-party node provider such as [Infura](https://infura.io/). You must also [create a wallet](https://www.myetherwallet.com/) and then obtain funds via the [Rinkeby Faucet](https://faucet.rinkeby.io/).

#### Install Node.js

(In case you don't have Node.js) There are a few ways to install Node.js. If you work on multiple JavaScript or TypeScript projects which require different `node` versions, using `nvm` may be suitable.

- [`nvm`](https://github.com/nvm-sh/nvm) (node version manager)
  - macOS: If you use [Homebrew](https://brew.sh/), run `brew install nvm` and you'll see some additional instructions. Follow it and `nvm install node` and `nvm use node`.
  - others: Refer to [`nvm`](https://github.com/nvm-sh/nvm) site.
- Install independently
  - macOS: If you use [Homebrew](https://brew.sh/), run `brew install node`
  - Ubuntu: `sudo apt-get install nodejs npm`

Then you should be able to use `npm`. To install `ganache-cli`, run `npm install -g ganache-cli`.

### XRP Settlement Engine

Interledger.rs and settlement engines written in other languages are fully interoperable. Here, we'll use the [XRP Ledger Settlement Engine](https://github.com/interledgerjs/settlement-xrp/), which is written in TypeScript. We'll need `node` and `npm` to install and run the settlement engine. If you don't have it already, refer to [Install Node.js](#install-nodejs).

Install the settlement engine as follows:

```bash #
npm i -g ilp-settlement-xrp
```

(This makes the `ilp-settlement-xrp` command available to your PATH.)

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

printf "Stopping Interledger nodes\n"

for port in `seq 6379 6385`; do
    if lsof -Pi :${port} -sTCP:LISTEN -t ; then
        redis-cli -p ${port} shutdown
    fi
done

if [ -f dump.rdb ] ; then
    rm -f dump.rdb
fi

for port in 8545 7770 8770 9770 3000 3001 3002 3003; do
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
redis-server --version &> /dev/null || error_and_exit "Uh oh! You need to install redis-server before running this example"
-->

```bash
# Create the logs directory if it doesn't already exist
mkdir -p logs

# Start Redis
redis-server --port 6379 &> logs/redis-a-node.log &
redis-server --port 6380 &> logs/redis-a-se-eth.log &
redis-server --port 6381 &> logs/redis-b-node.log &
redis-server --port 6382 &> logs/redis-b-se-eth.log &
redis-server --port 6383 &> logs/redis-b-se-xrp.log &
redis-server --port 6384 &> logs/redis-c-node.log &
redis-server --port 6385 &> logs/redis-c-se-xrp.log &
```

To remove all the data in Redis, you might additionally perform:

<!--!
sleep 2
printf "done\n"
-->
```bash
for port in `seq 6379 6385`; do
    redis-cli -p $port flushall
done
```

When you want to watch logs, use the `tail` command. You can use the command like: `tail -f logs/redis-alice.log`

### 3. Launch Ganache

This will launch an Ethereum testnet with 10 prefunded accounts. The mnemonic is used because we want to know the keys we'll use for Alice and Bob (otherwise they are randomized).

<!--!
printf "\nStarting local Ethereum testnet\n"
-->

```bash
ganache-cli -m "abstract vacuum mammal awkward pudding scene penalty purchase dinner depart evoke puzzle" -i 1 &> logs/ganache.log &
```

<!--!
sleep 3
-->

### 4. Launch Settlement Engines

In this example, we'll connect 3 Interledger nodes and each node needs its own settlement engine for each settlement ledger; We'll launch 4 settlement engines in total.

1. A settlement engine for Alice to Bob on Ethereum
   - To settle the balance of Bob's account on Alice's node (Port 3000)
1. A settlement engine for Bob to Alice on Ethereum
   - To settle the balance of Alice's account on Bob's node (Port 3001)
1. A settlement engine for Bob to Charlie on XRPL
   - To settle the balance of Charlie's account on Bob's node (Port 3002)
1. A settlement engine for Charlie to Bob on XRPL
   - To settle the balance of Bob's account on Charlie's node (Port 3003)

By default, the XRP settlement engine generates new testnet XRPL accounts prefunded with 1,000 testnet XRP (a new account is generated each run). Alternatively, you may supply an `XRP_SECRET` environment variable by generating your own testnet credentials from the [official faucet](https://xrpl.org/xrp-test-net-faucet.html).

The engines are part of a [separate repository](https://github.com/interledger-rs/settlement-engines) so you have to install another binary or compile from the source code.

#### Download Compiled Binaries

We also provide compiled binaries of `settlement-engines` for

- Linux based OSs
- macOS

##### Linux based OSs

<!--!
if [ ${SOURCE_MODE} -ne 1 ]; then
    if [ $(is_linux) -eq 1 ]; then
-->
```bash
pushd ~/.interledger/bin &>/dev/null

if [ ! -e "ilp-settlement-ethereum" ]; then
    curl -L https://github.com/interledger-rs/settlement-engines/releases/download/ilp-settlement-ethereum-latest/ilp-settlement-ethereum-x86_64-unknown-linux-musl.tar.gz | tar xzv
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

if [ ! -e "ilp-settlement-ethereum" ]; then
    curl -L https://github.com/interledger-rs/settlement-engines/releases/download/ilp-settlement-ethereum-latest/ilp-settlement-ethereum-x86_64-apple-darwin.tar.gz | tar xzv -
fi

popd &>/dev/null
```
<!--!
    fi
fi
-->

#### Compile from the Source Code

If you would prefer compiling from the source code and you've never cloned `settlement-engines`, the first step would be to clone the repository.

<!--!
if [ ${SOURCE_MODE} -eq 1 ]; then
    pushd ~/.interledger &>/dev/null
    if [ ! -e "settlement-engines" ]; then
        git clone https://github.com/interledger-rs/settlement-engines
    fi
    pushd settlement-engines &>/dev/null
-->

```bash #
# This should be done outside of the interledger-rs directory, otherwise it will cause an error.
git clone https://github.com/interledger-rs/settlement-engines
cd settlement-engines
```

Then install `settlement-engines` as follows.

<!--!
    printf "Building settlement-engines... (This may take a couple of minutes)\n"
-->
```bash
# This alias makes our command invocations more natural
alias ilp-settlement-ethereum="cargo run --quiet --bin ilp-settlement-ethereum --"

cargo build --bin ilp-settlement-ethereum
```
<!--!
    popd &>/dev/null
    popd &>/dev/null
fi
-->

#### Spin up

Then, spin up your settlement engines.

<!--!
printf "\nStarting settlement engines...\n"

if [ ${SOURCE_MODE} -eq 1 ]; then
    pushd ~/.interledger/settlement-engines &>/dev/null
fi
-->

```bash
# Turn on debug logging for all of the interledger.rs components
export RUST_LOG=interledger=debug
mkdir -p logs

# Start Alice's settlement engine (ETH)
ilp-settlement-ethereum \
--private_key 380eb0f3d505f087e438eca80bc4df9a7faa24f868e69fc0440261a0fc0567dc \
--confirmations 0 \
--poll_frequency 1000 \
--ethereum_url http://127.0.0.1:8545 \
--connector_url http://127.0.0.1:7771 \
--redis_url redis://127.0.0.1:6380/ \
--asset_scale 6 \
--settlement_api_bind_address 127.0.0.1:3000 \
&> logs/node-alice-settlement-engine-eth.log &

# Start Bob's settlement engine (ETH, XRPL)
ilp-settlement-ethereum \
--private_key cc96601bc52293b53c4736a12af9130abf347669b3813f9ec4cafdf6991b087e \
--confirmations 0 \
--poll_frequency 1000 \
--ethereum_url http://127.0.0.1:8545 \
--connector_url http://127.0.0.1:8771 \
--redis_url redis://127.0.0.1:6382/ \
--asset_scale 6 \
--settlement_api_bind_address 127.0.0.1:3001 \
&> logs/node-bob-settlement-engine-eth.log &

DEBUG="settlement*" \
CONNECTOR_URL="http://localhost:8771" \
REDIS_URI=127.0.0.1:6383 \
ENGINE_PORT=3002 \
ilp-settlement-xrp \
&> logs/node-bob-settlement-engine-xrpl.log &

# Start Charlie's settlement engine (XRPL)
DEBUG="settlement*" \
CONNECTOR_URL="http://localhost:9771" \
REDIS_URI=127.0.0.1:6385 \
ENGINE_PORT=3003 \
ilp-settlement-xrp \
&> logs/node-charlie-settlement-engine-xrpl.log &
```

<!--!
if [ ${SOURCE_MODE} -eq 1 ]; then
    popd &>/dev/null
fi
-->

### 5. Launch 3 Nodes

<!--!
printf "\nStarting nodes...\n"
-->

```bash
# Start nodes.
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

# Start Charlie's node. The --ilp_address field is omitted, so the node's address will 
# be `local.host`. When a parent account is added, Charlie's node will send an ILDCP request
# to them, which they will respond to with an address that they have assigned to Charlie.
# Charlie will then proceed to set that as their node's address.
# ILDCP documentation: https://interledger.org/rfcs/0031-dynamic-configuration-protocol/
ilp-node \
--secret_seed 1232362131122139900555208458637022875563691455429373719368053354 \
--admin_auth_token hi_charlie \
--redis_url redis://127.0.0.1:6384/ \
--http_bind_address 127.0.0.1:9770 \
--settlement_api_bind_address 127.0.0.1:9771 \
&> logs/node-charlie.log &
```

<!--!
printf "\nWaiting for nodes to start up"

wait_to_serve "http://localhost:7770" 10 || error_and_exit "\nFailed to spin up nodes. Check out your configuration and log files."
wait_to_serve "http://localhost:8770" 10 || error_and_exit "\nFailed to spin up nodes. Check out your configuration and log files."
wait_to_serve "http://localhost:9770" 10 || error_and_exit "\nFailed to spin up nodes. Check out your configuration and log files."
wait_to_serve "http://localhost:3000" 10 || error_and_exit "\nFailed to spin up settlement engine. Check out your configuration and log files."
wait_to_serve "http://localhost:3001" 10 || error_and_exit "\nFailed to spin up settlement engine. Check out your configuration and log files."
wait_to_serve "http://localhost:3002" 10 || error_and_exit "\nFailed to spin up settlement engine. Check out your configuration and log files."
wait_to_serve "http://localhost:3003" 10 || error_and_exit "\nFailed to spin up settlement engine. Check out your configuration and log files."

printf "done\nThe Interledger.rs nodes are up and running!\n\n"
-->

### 6. Configure the Nodes

<!--!
printf "Creating accounts:\n"
-->

```bash
# For authenticating to nodes, we can set credentials as an environment variable or a CLI argument
export ILP_CLI_API_AUTH=hi_alice

printf "Adding Alice's account...\n"
ilp-cli accounts create alice \
    --ilp-address example.alice \
    --asset-code ETH \
    --asset-scale 6 \
    --max-packet-amount 100 \
    --ilp-over-http-incoming-token alice_password \
    --settle-to 0 &> logs/account-alice-alice.log

printf "Adding Bob's account on Alice's node (ETH Peer relation)...\n"
ilp-cli accounts create bob \
    --ilp-address example.bob \
    --asset-code ETH \
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

printf "Adding Alice's account on Bob's node (ETH Peer relation)...\n"
ilp-cli --node http://localhost:8770 accounts create alice \
    --auth hi_bob \
    --ilp-address example.alice \
    --asset-code ETH \
    --asset-scale 6 \
    --max-packet-amount 100 \
    --settlement-engine-url http://localhost:3001 \
    --ilp-over-http-incoming-token alice_password \
    --ilp-over-http-outgoing-token bob_password \
    --ilp-over-http-url http://localhost:7770/accounts/bob/ilp \
    --settle-threshold 500 \
    --min-balance -1000 \
    --settle-to 0 \
    --routing-relation Peer &> logs/account-bob-alice.log

printf "Adding Charlie's account on Bob's node (XRP Child relation)...\n"
# you can optionally provide --ilp-address example.bob.charlie as an argument, 
# but the node is smart enough to figure it by itself
ilp-cli --node http://localhost:8770 accounts create charlie \
    --auth hi_bob \
    --asset-code XRP \
    --asset-scale 6 \
    --max-packet-amount 100 \
    --settlement-engine-url http://localhost:3002 \
    --ilp-over-http-incoming-token charlie_password \
    --ilp-over-http-outgoing-token bob_other_password \
    --ilp-over-http-url http://localhost:9770/accounts/bob/ilp \
    --settle-threshold 500 \
    --min-balance -1000 \
    --settle-to 0 \
    --routing-relation Child &> logs/account-bob-charlie.log &

printf "Adding Charlie's Account...\n"
# initially, Charlie gets added as local.host.charlie
ilp-cli --node http://localhost:9770 accounts create charlie \
    --auth hi_charlie \
    --asset-code XRP \
    --asset-scale 6 \
    --max-packet-amount 100 \
    --ilp-over-http-incoming-token charlie_password \
    --settle-to 0 &> logs/account-charlie-charlie.log

printf "Adding Bob's account on Charlie's node (XRP Parent relation)...\n"
# Once a parent is added, Charlie's node address is updated to `example.bob.charlie, 
# and then subsequently the addresses of all NonRoutingAccount and Child accounts get 
# updated to ${NODE_ADDRESS}.${CHILD_USERNAME}, with the exception of the account whose
# username matches the suffix of the node's address.
# So in this case, Charlie's account address gets updated from local.host.charlie to example.bob.charlie
# If Charlie had another Child account saved, say `alice`, Alice's address would become
# `example.bob.charlie.alice`
ilp-cli --node http://localhost:9770 accounts create bob \
    --auth hi_charlie \
    --ilp-address example.bob \
    --asset-code XRP \
    --asset-scale 6 \
    --max-packet-amount 100 \
    --settlement-engine-url http://localhost:3003 \
    --ilp-over-http-incoming-token bob_other_password \
    --ilp-over-http-outgoing-token charlie_password \
    --ilp-over-http-url http://localhost:8770/accounts/charlie/ilp \
    --settle-threshold 500 \
    --min-balance -1000 \
    --settle-to 0 \
    --routing-relation Parent &> logs/account-charlie-bob.log

sleep 2
```

Now three nodes and its settlement engines are set and accounts for each node are also set up.

Notice how we use Alice's settlement engine endpoint while registering Bob. This means that whenever Alice interacts with Bob's account, she'll use that Settlement Engine. This could be also said for the other accounts on the other nodes.

The `settle_threshold` and `settle_to` parameters control when settlements are triggered. The node will send a settlement when an account's balance reaches the `settle_threshold`, and it will settle for `balance - settle_to`.

### 7. Set the exchange rate between ETH and XRP on Bob's connector

```bash
printf "\nSetting the exchange rate...\n"
ilp-cli --node http://localhost:8770 rates set-all \
    --auth hi_bob \
    --pair ETH 1 \
    --pair XRP 1 \
    >/dev/null
```

### 8. Sending a Payment

<!--!
printf "\nChecking balances prior to payment...\n"

printf "\nAlice's balance on Alice's node: "
ilp-cli accounts balance alice

printf "Bob's balance on Alice's node: "
ilp-cli accounts balance bob

printf "Alice's balance on Bob's node: "
ilp-cli --node http://localhost:8770 accounts balance alice --auth hi_bob

printf "Charlie's balance on Bob's node: "
ilp-cli --node http://localhost:8770 accounts balance charlie --auth hi_bob

printf "Bob's balance on Charlie's node: "
ilp-cli --node http://localhost:9770 accounts balance bob --auth hi_charlie

printf "Charlie's balance on Charlie's node: "
ilp-cli --node http://localhost:9770 accounts balance charlie --auth hi_charlie

printf "\n\n"
-->

The following script sends a payment from Alice to Charlie through Bob.

<!--!
printf "Sending payment of 500 from Alice to Charlie through Bob\n"
-->

```bash
ilp-cli pay alice --auth alice_password \
    --amount 500 \
    --to http://localhost:9770/accounts/charlie/spsp
```

<!--!
printf "\n"

# wait untill the settlement is done
printf "\nWaiting for Ethereum block to be mined"
wait_to_get_http_response_body '{"asset_code":"ETH","balance":0.0}' 10 -H "Authorization: Bearer hi_bob" "http://localhost:8770/accounts/alice/balance" || error_and_exit "Could not confirm settlement."
printf "done\n"

printf "Waiting for XRP ledger to be validated"
wait_to_get_http_response_body '{"asset_code":"XRP","balance":0.0}' 20 -H "Authorization: Bearer hi_charlie" "http://localhost:9770/accounts/bob/balance" || error_and_exit "Could not confirm settlement."
printf "done\n"
-->

### 8. Check Balances

You may see unsettled balances before the settlement engines exactly work. Wait a few seconds and try later.

<!--!
printf "\nChecking balances after payment...\n"
-->
```bash
printf "\nAlice's balance on Alice's node: "
ilp-cli accounts balance alice

printf "Bob's balance on Alice's node: "
ilp-cli accounts balance bob

printf "Alice's balance on Bob's node: "
ilp-cli --node http://localhost:8770 accounts balance alice --auth hi_bob

printf "Charlie's balance on Bob's node: "
ilp-cli --node http://localhost:8770 accounts balance charlie --auth hi_bob

printf "Bob's balance on Charlie's node: "
ilp-cli --node http://localhost:9770 accounts balance bob --auth hi_charlie

printf "Charlie's balance on Charlie's node: "
ilp-cli --node http://localhost:9770 accounts balance charlie --auth hi_charlie
```

### 9. Kill All the Services

Finally, you can stop all the services as follows:

```bash #
for port in `seq 6379 6385`; do
    if lsof -Pi :${port} -sTCP:LISTEN -t >/dev/null ; then
        redis-cli -p ${port} shutdown
    fi
done

if [ -f dump.rdb ] ; then
    rm -f dump.rdb
fi

for port in 8545 7770 8770 9770 3000 3001 3002 3003; do
    if lsof -tPi :${port} >/dev/null ; then
        kill `lsof -tPi :${port}`
    fi
done
```

## Advanced

### Check the Settlement Block Generation

To check whether the settlement block is generated, we use `geth`. `geth` is the abbreviation of `go-ethereum` which is an Ethereum client written in the go language. If you don't already have `geth`, refer to the following.

- Compile and install from the source code
  - Refer to [Building Ethereum](https://github.com/ethereum/go-ethereum/wiki/Building-Ethereum) page.
- Install using package managers
  - Ubuntu: Follow the instructions [here](https://github.com/ethereum/go-ethereum/wiki/Installation-Instructions-for-Ubuntu).
  - macOS: If you use Homebrew, run `brew tap ethereum/ethereum` and `brew install ethereum`. Details are found [here](https://github.com/ethereum/go-ethereum/wiki/Installation-Instructions-for-Mac).
  - others: Refer to [Building Ethereum](https://github.com/ethereum/go-ethereum/wiki/Building-Ethereum) page.

Then dump transaction logs as follows. You will see generated block information. Be aware that ganache takes 10 to 20 seconds to generate a block. So you will have to wait for it before you check with `geth`.

<!-- # below means preventing output through run-md.sh -->

```bash #
printf "Last block: "
geth --exec "eth.getTransaction(eth.getBlock(eth.blockNumber-1).transactions[0])" attach http://localhost:8545 2>/dev/null
printf "\nCurrent block: "
geth --exec "eth.getTransaction(eth.getBlock(eth.blockNumber).transactions[0])" attach http://localhost:8545 2>/dev/null
```

If you inspect `ganache-cli`'s output, you will notice that the block number has increased as a result of the settlement executions as well.

### Check the Incoming Settlement on XRPL

You'll find incoming settlement logs in your settlement engine logs. Try:

```bash #
cat logs/node-charlie-settlement-engine-xrpl.log | grep "Received incoming XRP payment"
```

<!--!
printf "\n\nYou could try the following command to check if a block is generated.\nTo check, you'll need to install geth.\n\n"
printf "To check the last block:\n\n"
printf "\tgeth --exec \"eth.getTransaction(eth.getBlock(eth.blockNumber-1).transactions[0])\" attach http://localhost:8545 2>/dev/null\n\n"
printf "To check the current block:\n\n"
printf "\tgeth --exec \"eth.getTransaction(eth.getBlock(eth.blockNumber).transactions[0])\" attach http://localhost:8545 2>/dev/null\n\n"
printf "You could also try the following command to check if XRPL incoming payment is done.\n\n"
printf "\tcat logs/node-charlie-settlement-engine-xrpl.log | grep \"Received incoming XRP payment\"\n"
printf "\n"
run_post_test_hook
if [ $TEST_MODE -ne 1 ]; then
    prompt_yn "Do you want to kill the services? [Y/n] " "y"
fi
printf "\n"
if [ "$PROMPT_ANSWER" = "y" ] || [ $TEST_MODE -eq 1 ] ; then
    exec 2>/dev/null
    for port in `seq 6379 6385`; do
        if lsof -Pi :${port} -sTCP:LISTEN -t >/dev/null ; then
            redis-cli -p ${port} shutdown
        fi
    done

    if [ -f dump.rdb ] ; then
        rm -f dump.rdb
    fi

    for port in 8545 7770 8770 9770 3000 3001 3002 3003; do
        if lsof -tPi :${port} >/dev/null ; then
            kill `lsof -tPi :${port}`
        fi
    done
fi
-->

## Troubleshooting

```
# When installing ganache-cli
gyp ERR! find Python Python is not set from command line or npm configuration
gyp ERR! find Python Python is not set from environment variable PYTHON
gyp ERR! find Python checking if "python" can be used
gyp ERR! find Python - executable path is "/Users/xxxx/anaconda3/bin/python"
gyp ERR! find Python - version is "3.6.2"
gyp ERR! find Python - version is 3.6.2 - should be >=2.6.0 <3.0.0
gyp ERR! find Python - THIS VERSION OF PYTHON IS NOT SUPPORTED
gyp ERR! find Python checking if "python2" can be used
gyp ERR! find Python - "python2" is not in PATH or produced an error
```

If you see an error like the above, you have to install Python 2.7.

```
# When installing Node.js with apt-get
E: Unable to locate package nodejs
E: Unable to locate package npm
```

Try `sudo apt-get update`.

```
# When you try run-md.sh
Fatal: Failed to start the JavaScript console: api modules: Post http://localhost:8545: context deadline exceeded
```

It seems that you failed to install `ganache-cli`. Try to install it.

## Conclusion

This example showed an SPSP payment sent between three Interledger.rs nodes that settled using on-ledger Ethereum and XRPL transactions.

More examples that enhance your integration with ILP are coming soon!
