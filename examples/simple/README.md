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
printf "Stopping Interledger nodes\n"

if lsof -Pi :6379 -sTCP:LISTEN -t >/dev/null ; then
    redis-cli -p 6379 shutdown
fi

if [ -f dump.rdb ] ; then
    rm -f dump.rdb
fi

if lsof -tPi :7770 ; then
    kill `lsof -tPi :7770`
fi

if lsof -tPi :8770 ; then
    kill `lsof -tPi :8770`
fi

printf "\n"
-->

### 1. Build interledger.rs
First of all, let's build interledger.rs. (This may take a couple of minutes)

<!--! printf "Building interledger.rs... (This may take a couple of minutes)\n" -->
```bash
cargo build --bins
```

### 2. Launch Redis

<!--!
redis-server --version > /dev/null || printf "\e[31mUh oh! You need to install redis-server before running this example\e[m\n"
-->

```bash
# Create the logs directory if it doesn't already exist
mkdir -p logs

# Start Redis
redis-server &> logs/redis.log &
```

When you want to watch logs, use the `tail` command. You can use the command like: `tail -f logs/redis.log`

### 3. Launch 2 Nodes

```bash
# Turn on debug logging for all of the interledger.rs components
export RUST_LOG=interledger=debug

# Start both nodes
# Note that the configuration options can be passed as environment variables
# or saved to a YAML file and passed to the node with the `--config` or `-c` CLI argument
ILP_ADDRESS=example.node_a \
ILP_SECRET_SEED=8852500887504328225458511465394229327394647958135038836332350604 \
ILP_ADMIN_AUTH_TOKEN=admin-a \
ILP_REDIS_CONNECTION=redis://127.0.0.1:6379/0 \
ILP_HTTP_ADDRESS=127.0.0.1:7770 \
ILP_BTP_ADDRESS=127.0.0.1:7768 \
ILP_SETTLEMENT_ADDRESS=127.0.0.1:7771 \
cargo run --package interledger -- node &> logs/node_a.log &

ILP_ADDRESS=example.node_b \
ILP_SECRET_SEED=1604966725982139900555208458637022875563691455429373719368053354 \
ILP_ADMIN_AUTH_TOKEN=admin-b \
ILP_REDIS_CONNECTION=redis://127.0.0.1:6379/1 \
ILP_HTTP_ADDRESS=127.0.0.1:8770 \
ILP_BTP_ADDRESS=127.0.0.1:8768 \
ILP_SETTLEMENT_ADDRESS=127.0.0.1:8771 \
cargo run --package interledger -- node &> logs/node_b.log &
```

<!--!
printf "\nWaiting for Interledger.rs nodes to start up...\n"

function wait_to_serve() {
    while :
    do
        printf "."
        sleep 1
        curl $1 &> /dev/null
        if [ $? -eq 0 ]; then
            break;
        fi
    done
}

wait_to_serve "http://localhost:7770"
wait_to_serve "http://localhost:8770"
printf "\n"

printf "The Interledger.rs nodes are up and running!\n\n"
-->

Now the Interledger.rs nodes are up and running!  
You can also watch the logs with: `tail -f logs/node_a.log` or `tail -f logs/node_b.log`.

### 4. Configure the Nodes

Let's create accounts on both nodes. The following script sets up accounts for two users, Alice and Bob. It also creates accounts that represent the connection between Nodes A and B.

See the [HTTP API docs](../../docs/api.md) for the full list of fields that can be set on an account.

<!--! printf "Creating accounts:\n\n" -->

```bash
# Insert accounts on Node A
# One account represents Alice and the other represents Node B's account with Node A

printf "Alice's account:\n"
curl \
    -H "Content-Type: application/json" \
    -H "Authorization: Bearer admin-a" \
    -d '{
    "ilp_address": "example.node_a.alice",
    "username" : "alice",
    "asset_code": "ABC",
    "asset_scale": 9,
    "max_packet_amount": 100,
    "http_incoming_token": "alice-password"}' \
    http://localhost:7770/accounts

printf "\nNode B's account on Node A:\n"
curl \
    -H "Content-Type: application/json" \
    -H "Authorization: Bearer admin-a" \
    -d '{
    "ilp_address": "example.node_b",
    "username" : "node_b",
    "asset_code": "ABC",
    "asset_scale": 9,
    "max_packet_amount": 100,
    "http_incoming_token": "node_b-password",
    "http_outgoing_token": "node_a:node_a-password",
    "http_endpoint": "http://localhost:8770/ilp",
    "min_balance": -100000,
    "routing_relation": "Peer",
    "send_routes": true,
    "receive_routes": true}' \
    http://localhost:7770/accounts

# Insert accounts on Node B
# One account represents Bob and the other represents Node A's account with Node B

printf "\nBob's Account:\n"
curl \
    -H "Content-Type: application/json" \
    -H "Authorization: Bearer admin-b" \
    -d '{
    "ilp_address": "example.node_b.bob",
    "username" : "bob",
    "asset_code": "ABC",
    "asset_scale": 9,
    "max_packet_amount": 100,
    "http_incoming_token": "bob"}' \
    http://localhost:8770/accounts

printf "\nNode A's account on Node B:\n"
curl \
    -H "Content-Type: application/json" \
    -H "Authorization: Bearer admin-b" \
    -d '{
    "ilp_address": "example.node_a",
    "username" : "node_a",
    "asset_code": "ABC",
    "asset_scale": 9,
    "max_packet_amount": 100,
    "http_incoming_token": "node_a-password",
    "http_outgoing_token": "node_b:node_b-password",
    "http_endpoint": "http://localhost:7770/ilp",
    "min_balance": -100000,
    "routing_relation": "Peer",
    "send_routes": true,
    "receive_routes": true}' \
    http://localhost:8770/accounts
```

<!--! printf "\n\nCreated accounts on both nodes\n\n" -->

### 5. Sending a Payment

<!--!
# check balances before payment
printf "Checking balances...\n"
printf "\nAlice's balance: "
curl \
-H "Authorization: Bearer admin-a" \
http://localhost:7770/accounts/alice/balance

printf "\nNode B's balance on Node A: "
curl \
-H "Authorization: Bearer admin-a" \
http://localhost:7770/accounts/node_b/balance

printf "\nNode A's balance on Node B: "
curl \
-H "Authorization: Bearer admin-b" \
http://localhost:8770/accounts/node_a/balance

printf "\nBob's balance: "
curl \
-H "Authorization: Bearer admin-b" \
http://localhost:8770/accounts/bob/balance

printf "\n\n"
-->

The following script sends a payment from Alice to Bob that is routed from Node A to Node B.

<!--! printf "Sending payment of 500 from Alice (on Node A) to Bob (on Node B)\n" -->

```bash
# Sending payment of 500 from Alice (on Node A) to Bob (on Node B)
curl \
    -H "Authorization: Bearer alice:alice-password" \
    -H "Content-Type: application/json" \
    -d '{"receiver":"http://localhost:8770/spsp/bob","source_amount":500}' \
    http://localhost:7770/pay
```

<!--! printf "\n\n" -->

### 6. Check Balances

You can run the following script to print each of the accounts' balances (try doing this before and after sending a payment).

<!--! printf "Checking balances...\n" -->

```bash
printf "\nAlice's balance: "
curl \
-H "Authorization: Bearer admin-a" \
http://localhost:7770/accounts/alice/balance

printf "\nNode B's balance on Node A: "
curl \
-H "Authorization: Bearer admin-a" \
http://localhost:7770/accounts/node_b/balance

printf "\nNode A's balance on Node B: "
curl \
-H "Authorization: Bearer admin-b" \
http://localhost:8770/accounts/node_a/balance

printf "\nBob's balance: "
curl \
-H "Authorization: Bearer admin-b" \
http://localhost:8770/accounts/bob/balance
```

<!--! printf "\n\n" -->

### 7. Kill All the Services
Finally, you can stop all the services as follows:

<!--! printf "Stopping Interledger nodes\n" -->

```bash
if lsof -Pi :6379 -sTCP:LISTEN -t >/dev/null ; then
    redis-cli -p 6379 flushall
    redis-cli -p 6379 shutdown
fi

if [ -f dump.rdb ] ; then
    rm -f dump.rdb
fi

if lsof -tPi :7770 ; then
    kill `lsof -tPi :7770`
fi

if lsof -tPi :8770 ; then
    kill `lsof -tPi :8770`
fi
```

<!--! printf "\n" -->

## Troubleshooting

> Uh oh! You need to install redis-server before running this example

You need to install Redis to run `redis-server` command. See [Prerequisites](#prerequisites) section.

> curl: (7) Failed to connect to localhost port 7770: Connection refused

Your interledger.rs node is not running. The reason may be:

1. You tried to insert the accounts before the nodes had time to spin up. Adjust the `sleep 1` command to `sleep 3` or larger.
1. You have already running interledger.rs nodes on port `7770`. Stop the nodes and retry.
1. You have some other process running on port `7770`. Stop the process and retry.

To stop the process running on port `7770`, try `` kill `lsof -i:7770 -t` ``. Since this example launches 2 nodes, the port may be other than `7770`. Adjust the port number according to the situation.

## Conclusion

That's it for this example! You've learned how to set up Interledger.rs nodes, connect them together, and how to send a payment from one to the other.

Check out the [other examples](../README.md) for more complex demos that show other features of Interledger, including settlement, multi-hop routing, and cross-currency payments.
