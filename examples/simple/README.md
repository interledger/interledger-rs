<!--!
# For integration tests
function post_test_hook() {
    if [ $TEST_MODE -eq 1 ]; then
        test_equals_or_exit '{"asset_code":"ABC","balance":-5e-7}' test_http_response_body -H "Authorization: Bearer admin-a" http://localhost:7770/accounts/alice/balance
        test_equals_or_exit '{"asset_code":"ABC","balance":5e-7}' test_http_response_body -H "Authorization: Bearer admin-a" http://localhost:7770/accounts/node_b/balance
        test_equals_or_exit '{"asset_code":"ABC","balance":-5e-7}' test_http_response_body -H "Authorization: Bearer admin-b" http://localhost:8770/accounts/node_a/balance
        test_equals_or_exit '{"asset_code":"ABC","balance":5e-7}' test_http_response_body -H "Authorization: Bearer admin-b" http://localhost:8770/accounts/bob/balance
    fi
}
-->

# Simple Two-Node Interledger Payment
> A demo of sending a payment between 2 Interledger.rs nodes without settlement.

## Overview

This example sets up two local Interledger.rs nodes, peers them together, and sends a payment from one to the other. This example does not involve any remote nodes or networks. 

To run the full example, you can use [`run-md.sh`](../../scripts/run-md.sh) as described [here](../README.md). Otherwise, you can walk through each step below.

Each of the services write their logs to files found under the `logs` directory. You can run `tail -f logs/node_a.log`, for example, to watch the logs of Node A.

![overview](images/overview.svg)

## Prerequisites

- [Rust](#rust)
- [Redis](#redis)

### Rust

Because Interledger.rs is written in the Rust language, you need the Rust environment. Refer to the [Getting started](https://www.rust-lang.org/learn/get-started) page or just `curl https://sh.rustup.rs -sSf | sh` and follow the instruction.

### Redis
The Interledger.rs nodes currently use [Redis](https://redis.io/) to store their data (SQL database support coming soon!)

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

for port in 6379 6380; do
    if lsof -Pi :${port} -sTCP:LISTEN -t ; then
        redis-cli -p ${port} shutdown
    fi
done

if [ -f dump.rdb ] ; then
    rm -f dump.rdb
fi

for port in 8545 7770 8770; do
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
redis-server --port 6380 &> logs/redis-b-node.log &
```

To remove all the data in Redis, you might additionally perform:

<!--!
sleep 2
printf "done\n"
-->
```bash
for port in `seq 6379 6380`; do
    redis-cli -p $port flushall
done
```

When you want to watch logs, use the `tail` command. You can use the command like: `tail -f logs/redis-a-node.log`

### 3. Launch 2 Nodes

<!--!
printf "\n\nStarting Interledger nodes...\n"
-->

```bash
# Turn on debug logging for all of the interledger.rs components
export RUST_LOG=interledger=debug

# Start both nodes.
# Note that the configuration options can be passed as environment variables
# or saved to a YAML, JSON or TOML file and passed to the node as a positional argument.
# You can also pass it from STDIN.

ilp-node \
--ilp_address example.node_a \
--secret_seed 8852500887504328225458511465394229327394647958135038836332350604 \
--admin_auth_token admin-a \
--redis_url redis://127.0.0.1:6379/ \
--http_bind_address 127.0.0.1:7770 \
--settlement_api_bind_address 127.0.0.1:7771 \
&> logs/node_a.log &

ilp-node \
--ilp_address example.node_b \
--secret_seed 1604966725982139900555208458637022875563691455429373719368053354 \
--admin_auth_token admin-b \
--redis_url redis://127.0.0.1:6380/ \
--http_bind_address 127.0.0.1:8770 \
--settlement_api_bind_address 127.0.0.1:8771 \
&> logs/node_b.log &
```

<!--!
printf "\nWaiting for Interledger.rs nodes to start up"

wait_to_serve "http://localhost:7770" 10 || error_and_exit "\nFailed to spin up nodes. Check out your configuration and log files."
wait_to_serve "http://localhost:8770" 10 || error_and_exit "\nFailed to spin up nodes. Check out your configuration and log files."

printf " done\nThe Interledger.rs nodes are up and running!\n\n"
-->

Now the Interledger.rs nodes are up and running!  
You can also watch the logs with: `tail -f logs/node_a.log` or `tail -f logs/node_b.log`.

### 4. Configure the Nodes

Let's create accounts on both nodes. The following script sets up accounts for two users, Alice and Bob. It also creates accounts that represent the connection between Nodes A and B.

Now that the nodes are up and running, we'll be using the `ilp-cli` command-line tool to work with them. This tool offers a convenient way for developers to inspect and interact with live nodes. Note that `ilp-cli` speaks to Interledger.rs nodes via the normal HTTP API, so you could also use any other HTTP client (such as `curl`, for example) to perform the same operations.

See the [HTTP API docs](../../docs/api.md) for the full list of fields that can be set on an account.

<!--!
printf "\nCreating accounts...\n\n"
-->

```bash
# For authenticating to nodes, we can set credentials as an environment variable or a CLI argument
export ILP_CLI_API_AUTH=admin-a

# Insert accounts on Node A
# One account represents Alice and the other represents Node B's account with Node A

printf "Creating Alice's account on Node A...\n"
ilp-cli accounts create alice \
    --asset-code ABC \
    --asset-scale 9 \
    --ilp-over-http-incoming-token alice-password \
    &>logs/account-node_a-alice.log

printf "Creating Node B's account on Node A...\n"
ilp-cli accounts create node_b \
    --asset-code ABC \
    --asset-scale 9 \
    --ilp-address example.node_b \
    --ilp-over-http-outgoing-token node_a-password \
    --ilp-over-http-url 'http://localhost:8770/accounts/node_a/ilp' \
    &>logs/account-node_a-node_b.log

# Insert accounts on Node B
# One account represents Bob and the other represents Node A's account with Node B

printf "Creating Bob's account on Node B...\n"
ilp-cli --node http://localhost:8770 accounts create bob \
    --auth admin-b \
    --asset-code ABC \
    --asset-scale 9 \
    &>logs/account-node_b-bob.log

printf "Creating Node A's account on Node B...\n"
ilp-cli --node http://localhost:8770 accounts create node_a \
    --auth admin-b \
    --asset-code ABC \
    --asset-scale 9 \
    --ilp-over-http-incoming-token node_a-password \
    &>logs/account-node_b-node_a.log
```

### 5. Sending a Payment

<!--!
printf "\nChecking balances prior to payment...\n"

printf "\nAlice's balance: "
ilp-cli accounts balance alice

printf "Node B's balance on Node A: "
ilp-cli accounts balance node_b

printf "Node A's balance on Node B: "
ilp-cli --node http://localhost:8770 accounts balance node_a --auth admin-b

printf "Bob's balance: "
ilp-cli --node http://localhost:8770 accounts balance bob --auth admin-b
    
printf "\n\n"
-->

The following command sends a payment from Alice to Bob that is routed from Node A to Node B.

<!--!
printf "Sending payment of 500 from Alice (on Node A) to Bob (on Node B)...\n\n"
-->

```bash
# Sending payment of 500 from Alice (on Node A) to Bob (on Node B)
ilp-cli pay alice \
    --auth alice-password \
    --amount 500 \
    --to http://localhost:8770/accounts/bob/spsp
```

<!--!
printf "\n"
-->

### 6. Check Balances

You can run the following script to print each of the accounts' balances (try doing this before and after sending a payment).

<!--!
printf "\nChecking balances after payment...\n"
-->

```bash
printf "\nAlice's balance: "
ilp-cli accounts balance alice

printf "Node B's balance on Node A: "
ilp-cli accounts balance node_b

printf "Node A's balance on Node B: "
ilp-cli --node http://localhost:8770 accounts balance node_a --auth admin-b 

printf "Bob's balance: "
ilp-cli --node http://localhost:8770 accounts balance bob --auth admin-b 
```
<!--!
printf "\n\n"
-->

### 7. Kill All the Services
Finally, you can stop all the services as follows:

<!--!
run_post_test_hook
if [ $TEST_MODE -ne 1 ]; then
    prompt_yn "Do you want to kill the services? [Y/n] " "y"
fi
printf "\n"
if [ "$PROMPT_ANSWER" = "y" ] || [ $TEST_MODE -eq 1 ] ; then
    exec 2> /dev/null
-->
```bash
for port in 6379 6380; do
    if lsof -Pi :${port} -sTCP:LISTEN -t >/dev/null ; then
        redis-cli -p ${port} shutdown
    fi
done

if [ -f dump.rdb ] ; then
    rm -f dump.rdb
fi

for port in 8545 7770 8770; do
    if lsof -tPi :${port} >/dev/null ; then
        kill `lsof -tPi :${port}` > /dev/null
    fi
done
```

<!--!
fi
printf "\n"
-->

## Troubleshooting

```
Uh oh! You need to install redis-server before running this example
```

You need to install Redis to run `redis-server` command. See [Prerequisites](#prerequisites) section.

```
curl: (7) Failed to connect to localhost port 7770: Connection refused
```

Your interledger.rs node is not running. The reason may be:

1. You tried to insert the accounts before the nodes had time to spin up. Wait a second or so, and try again.
1. You have already running interledger.rs nodes on port `7770`. Stop the nodes and retry.
1. You have some other process running on port `7770`. Stop the process and retry.

To stop the process running on port `7770`, try `` kill `lsof -i:7770 -t` ``. Since this example launches 2 nodes, the port may be other than `7770`. Adjust the port number according to the situation.

## Conclusion

That's it for this example! You've learned how to set up Interledger.rs nodes, connect them together, and how to send a payment from one to the other.

Check out the [other examples](../README.md) for more complex demos that show other features of Interledger, including settlement, multi-hop routing, and cross-currency payments.
