# Interledger with Ethereum and XRP On-Ledger Settlement
> A demo that sends payments between 3 Interledger.rs nodes and settles using Ethereum transactions and XRP transactions.

## Overview
This example shows how to configure Interledger.rs nodes and use an Ethereum network (testnet or mainnet) and the XRP Ledger testnet as settlement ledgers for payments sent between the nodes. If you are new to Ethereum, you can learn about it [here](https://www.ethereum.org/beginners/). You'll find many useful resources about the XRP Ledger (XRPL) [here](https://xrpl.org). To learn about settlement in Interledger, refer to [Peering, Clearing and Settling](https://github.com/interledger/rfcs/blob/master/0032-peering-clearing-settlement/0032-peering-clearing-settlement.md).

![overview](images/overview.svg)

## Prerequisites

- [Docker](#docker)
- [An Ethereum network](#an-ethereum-network) to connect to

### Docker
Because we provide docker images for node and others on [Docker Hub](https://hub.docker.com/u/interledgerrs), in this example we utilize it. You have to install Docker if you don't have already.

- Ubuntu: [Get Docker Engine - Community for Ubuntu](https://docs.docker.com/install/linux/docker-ce/ubuntu/)
- macOS: [Install Docker Desktop for Mac](https://docs.docker.com/docker-for-mac/install/)

### An Ethereum network
First, you need an Ethereum network. You can either use a local testnet, a remote testnet, or the mainnet.

For this example, we'll use [ganache-cli](https://github.com/trufflesuite/ganache-cli) which deploys a local Ethereum testnet at `localhost:8545`. To install `ganache-cli`, run `npm install -g ganache-cli`. If you do not already have Node.js installed on your computer, you can follow [the instructions below](#install-nodejs) to install it.

Advanced: You can run this against the Rinkeby Testnet by running a node that connects to Rinkeby (e.g. `geth --rinkeby --syncmode "light"`) or use a third-party node provider such as [Infura](https://infura.io/). You must also [create a wallet](https://www.myetherwallet.com/) and then obtain funds via the [Rinkeby Faucet](https://faucet.rinkeby.io/).

## Instructions

<!--!
function error_and_exit() {
    printf "\e[31m$1\e[m\n"
    exit 1
}

docker --version > /dev/null || error_and_exit "Uh oh! You need to install Docker before running this example"

printf "Stopping Interledger nodes\n"

REDIS_CONTAINER_ID=`docker ps -q -f "name=redis_"`
for container_id in ${REDIS_CONTAINER_ID}; do
    docker exec ${container_id} redis-cli flushall
done

docker stop \
    interledger-rs-node_a \
    interledger-rs-node_b \
    interledger-rs-node_c \
    interledger-rs-se_a \
    interledger-rs-se_b \
    interledger-rs-se_c \
    interledger-rs-se_d \
    redis_a \
    redis_b \
    redis_c \
    ganache 2> /dev/null
-->

### 0. Clean up Docker
If you have ever tried the other examples, clean up your docker because the names may conflict.

```bash #
docker stop `docker ps -aq -f "name=interledger-rs-node"`
docker rm `docker ps -aq -f "name=interledger-rs-node"`

docker stop `docker ps -aq -f "name=interledger-rs-se_"`
docker rm `docker ps -aq -f "name=interledger-rs-se_"`

docker stop `docker ps -aq -f "name=redis"`
docker rm `docker ps -aq -f "name=redis"`
```

### 1. Set up Docker
First, let's set up a docker network so that containers could connect each other.

<!--!
NETWORK_ID=`docker network ls -f "name=interledger" --format="{{.ID}}"`
if [ -z "${NETWORK_ID}" ]; then
    printf "Creating a docker network...\n"
    docker network create interledger
fi
-->
```bash #
docker network create interledger
```

### 2. Launch Redis

<!--!
printf "\nStarting Redis...\n"
-->

```bash
docker start redis_a || docker run --name redis_a -d -p 6379:6379 --network=interledger redis:5.0.5
docker start redis_b || docker run --name redis_b -d -p 6380:6379 --network=interledger redis:5.0.5
docker start redis_c || docker run --name redis_c -d -p 6381:6379 --network=interledger redis:5.0.5
```

When you want to watch logs, use the `docker logs` command. You can use the command like: `docker logs redis`.

### 3. Launch Ganache

This will launch an Ethereum testnet with 10 prefunded accounts. The mnemonic is used because we want to know the keys we'll use for Alice and Bob (otherwise they are randomized).

<!--!
printf "\nStarting local Ethereum testnet\n"
-->

```bash
docker start ganache ||
docker run \
        -p 8545:8545 \
        --network=interledger \
        --name=ganache \
        -id \
        trufflesuite/ganache-cli \
        -h 0.0.0.0 \
        -p 8545 \
        -m "abstract vacuum mammal awkward pudding scene penalty purchase dinner depart evoke puzzle" \
        -i 1
```

<!--! sleep 3 -->

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

Instead of using the LEDGER_ADDRESS and LEDGER_SECRET from the examples below, you can generate your own XRPL credentials at the [official faucet](https://xrpl.org/xrp-test-net-faucet.html).

<!--!
printf "\nStarting settlement engines...\n"
-->

```bash
# Start Alice's settlement engine (ETH)
docker start interledger-rs-se_a ||
docker run \
    -p 3000:3000 \
    --network=interledger \
    --name=interledger-rs-se_a \
    -id \
    interledgerrs/settlement-engine ethereum-ledger \
    --key 380eb0f3d505f087e438eca80bc4df9a7faa24f868e69fc0440261a0fc0567dc \
    --confirmations 0 \
    --poll_frequency 1000 \
    --ethereum_endpoint http://ganache:8545 \
    --connector_url http://interledger-rs-node_a:7771 \
    --redis_uri redis://redis_a:6379/0 \
    --asset_scale 6 \
    --http_address 0.0.0.0:3000

# Start Bob's settlement engine (ETH, XRPL)
docker start interledger-rs-se_b ||
docker run \
    -p 3001:3000 \
    --network=interledger \
    --name=interledger-rs-se_b \
    -id \
    interledgerrs/settlement-engine ethereum-ledger \
    --key cc96601bc52293b53c4736a12af9130abf347669b3813f9ec4cafdf6991b087e \
    --confirmations 0 \
    --poll_frequency 1000 \
    --ethereum_endpoint http://ganache:8545 \
    --connector_url http://interledger-rs-node_b:7771 \
    --redis_uri redis://redis_b:6379/0 \
    --asset_scale 6 \
    --http_address 0.0.0.0:3000

docker start interledger-rs-se_c ||
docker run \
    -p 3002:3000 \
    --network=interledger \
    --name=interledger-rs-se_c \
    -id \
    -e DEBUG=xrp-settlement-engine \
    -e LEDGER_ADDRESS=r4f94XM93wMXdmnUg9hkE4maq8kryD9Yhj \
    -e LEDGER_SECRET=snun48LX8WMtTJEzHZ4ckbiEf4dVZ \
    -e CONNECTOR_URL=http://interledger-rs-node_b:7771 \
    -e REDIS_HOST=redis_b \
    -e REDIS_PORT=6379 \
    -e ENGINE_PORT=3000 \
    interledgerjs/settlement-xrp

# Start Charlie's settlement engine (XRPL)
docker start interledger-rs-se_d ||
docker run \
    -p 3003:3000 \
    --network=interledger \
    --name=interledger-rs-se_d \
    -id \
    -e DEBUG=xrp-settlement-engine \
    -e LEDGER_ADDRESS=rPh1Bc42tjeg3waow1m1dzo31mBoaqTDjj \
    -e LEDGER_SECRET=ssxRAdmN97CnEjSzBfYFVCKiw2wNM \
    -e CONNECTOR_URL=http://interledger-rs-node_c:7771 \
    -e REDIS_HOST=redis_c \
    -e REDIS_PORT=6379 \
    -e ENGINE_PORT=3000 \
    interledgerjs/settlement-xrp
```

### 5. Launch 3 Nodes

<!--!
printf "\nStarting nodes...\n"
-->

```bash
# Start Alice's node
docker start interledger-rs-node_a ||
docker run \
    -e ILP_ADDRESS=example.alice \
    -e ILP_SECRET_SEED=8852500887504328225458511465394229327394647958135038836332350604 \
    -e ILP_ADMIN_AUTH_TOKEN=hi_alice \
    -e ILP_REDIS_CONNECTION=redis://redis_a:6379/0 \
    -e ILP_HTTP_ADDRESS=0.0.0.0:7770 \
    -e ILP_BTP_ADDRESS=0.0.0.0:7768 \
    -e ILP_SETTLEMENT_ADDRESS=0.0.0.0:7771 \
    -p 7768:7768 \
    -p 7770:7770 \
    -p 7771:7771 \
    --network=interledger \
    --name=interledger-rs-node_a \
    -id \
    interledgerrs/node node

# Start Bob's node
docker start interledger-rs-node_b ||
docker run \
    -e ILP_ADDRESS=example.bob \
    -e ILP_SECRET_SEED=1604966725982139900555208458637022875563691455429373719368053354 \
    -e ILP_ADMIN_AUTH_TOKEN=hi_bob \
    -e ILP_REDIS_CONNECTION=redis://redis_b:6379/0 \
    -e ILP_HTTP_ADDRESS=0.0.0.0:7770 \
    -e ILP_BTP_ADDRESS=0.0.0.0:7768 \
    -e ILP_SETTLEMENT_ADDRESS=0.0.0.0:7771 \
    -p 8768:7768 \
    -p 8770:7770 \
    -p 8771:7771 \
    --network=interledger \
    --name=interledger-rs-node_b \
    -id \
    interledgerrs/node node

# Start Charlie's node
docker start interledger-rs-node_c ||
docker run \
    -e ILP_ADDRESS=example.charlie \
    -e ILP_SECRET_SEED=1232362131122139900555208458637022875563691455429373719368053354 \
    -e ILP_ADMIN_AUTH_TOKEN=hi_charlie \
    -e ILP_REDIS_CONNECTION=redis://redis_c:6379/0 \
    -e ILP_HTTP_ADDRESS=0.0.0.0:7770 \
    -e ILP_BTP_ADDRESS=0.0.0.0:7768 \
    -e ILP_SETTLEMENT_ADDRESS=0.0.0.0:7771 \
    -p 9768:7768 \
    -p 9770:7770 \
    -p 9771:7771 \
    --network=interledger \
    --name=interledger-rs-node_c \
    -id \
    interledgerrs/node node
```

<!--!
printf "\nWaiting for nodes to start up...\n"

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
wait_to_serve "http://localhost:9770"
wait_to_serve "http://localhost:3000"
wait_to_serve "http://localhost:3001"
wait_to_serve "http://localhost:3002"
wait_to_serve "http://localhost:3003"
printf "\n"
-->

### 6. Configure the Nodes

<!--! printf "Creating accounts:\n" -->

```bash
# Adding settlement accounts should be done at the same time because it checks each other

printf "Adding Alice's account...\n"
curl \
    -H "Content-Type: application/json" \
    -H "Authorization: Bearer hi_alice" \
    -d '{
    "username" : "alice",
    "ilp_address": "example.alice",
    "asset_code": "ETH",
    "asset_scale": 6,
    "max_packet_amount": 100,
    "http_incoming_token": "alice_password",
    "http_endpoint": "http://interledger-rs-node_a:7770/ilp",
    "settle_to" : 0}' \
    http://localhost:7770/accounts > logs/account-alice-alice.log 2>/dev/null

printf "Adding Charlie's Account...\n"
curl \
    -H "Content-Type: application/json" \
    -H "Authorization: Bearer hi_charlie" \
    -d '{
    "ilp_address": "example.bob.charlie",
    "username" : "charlie",
    "asset_code": "XRP",
    "asset_scale": 6,
    "max_packet_amount": 100,
    "http_incoming_token": "charlie_password",
    "http_endpoint": "http://interledger-rs-node_c:7770/ilp",
    "settle_to" : 0}' \
    http://localhost:9770/accounts > logs/account-charlie-charlie.log 2>/dev/null

printf "Adding Bob's account on Alice's node (ETH Peer relation)...\n"
curl \
    -H "Content-Type: application/json" \
    -H "Authorization: Bearer hi_alice" \
    -d '{
    "ilp_address": "example.bob",
    "username" : "bob",
    "asset_code": "ETH",
    "asset_scale": 6,
    "max_packet_amount": 100,
    "settlement_engine_url": "http://interledger-rs-se_a:3000",
    "http_incoming_token": "bob_password",
    "http_outgoing_token": "alice:alice_password",
    "http_endpoint": "http://interledger-rs-node_b:7770/ilp",
    "settle_threshold": 500,
    "min_balance": -1000,
    "settle_to" : 0,
    "routing_relation": "Peer",
    "send_routes": true,
    "receive_routes": true}' \
    http://localhost:7770/accounts > logs/account-alice-bob.log 2>/dev/null &

printf "Adding Alice's account on Bob's node (ETH Peer relation)...\n"
curl \
    -H "Content-Type: application/json" \
    -H "Authorization: Bearer hi_bob" \
    -d '{
    "ilp_address": "example.alice",
    "username": "alice",
    "asset_code": "ETH",
    "asset_scale": 6,
    "max_packet_amount": 100,
    "settlement_engine_url": "http://interledger-rs-se_b:3000",
    "http_incoming_token": "alice_password",
    "http_outgoing_token": "bob:bob_password",
    "http_endpoint": "http://interledger-rs-node_a:7770/ilp",
    "settle_threshold": 500,
    "min_balance": -1000,
    "settle_to" : 0,
    "routing_relation": "Peer",
    "send_routes": true,
    "receive_routes": true}' \
    http://localhost:8770/accounts > logs/account-bob-alice.log 2>/dev/null

printf "Adding Charlie's account on Bob's node (XRP Child relation)...\n"
curl \
    -H "Content-Type: application/json" \
    -H "Authorization: Bearer hi_bob" \
    -d '{
    "ilp_address": "example.bob.charlie",
    "username" : "charlie",
    "asset_code": "XRP",
    "asset_scale": 6,
    "max_packet_amount": 100,
    "settlement_engine_url": "http://interledger-rs-se_c:3000",
    "http_incoming_token": "charlie_password",
    "http_outgoing_token": "bob:bob_other_password",
    "http_endpoint": "http://interledger-rs-node_c:7770/ilp",
    "settle_threshold": 500,
    "min_balance": -1000,
    "settle_to" : 0,
    "routing_relation": "Child",
    "send_routes": false,
    "receive_routes": true}' \
    http://localhost:8770/accounts > logs/account-bob-charlie.log 2>/dev/null &

printf "Adding Bob's account on Charlie's node (XRP Parent relation)...\n"
curl \
    -H "Content-Type: application/json" \
    -H "Authorization: Bearer hi_charlie" \
    -d '{
    "ilp_address": "example.bob",
    "username" : "bob",
    "asset_code": "XRP",
    "asset_scale": 6,
    "max_packet_amount": 100,
    "settlement_engine_url": "http://interledger-rs-se_d:3000",
    "http_incoming_token": "bob_other_password",
    "http_outgoing_token": "charlie:charlie_password",
    "http_endpoint": "http://interledger-rs-node_b:7770/ilp",
    "settle_threshold": 500,
    "min_balance": -1000,
    "settle_to" : 0,
    "routing_relation": "Parent",
    "send_routes": false,
    "receive_routes": true}' \
    http://localhost:9770/accounts > logs/account-charlie-bob.log 2>/dev/null

sleep 2
```

Now three nodes and its settlement engines are set and accounts for each node are also set up.

Notice how we use Alice's settlement engine endpoint while registering Bob. This means that whenever Alice interacts with Bob's account, she'll use that Settlement Engine. This could be also said for the other accounts on the other nodes.

The `settle_threshold` and `settle_to` parameters control when settlements are triggered. The node will send a settlement when an account's balance reaches the `settle_threshold`, and it will settle for `balance - settle_to`.

### 7. Set the exchange rate between ETH and XRP on Bob's connector

```bash
printf "\nSetting the exchange rate...\n"
curl http://localhost:8770/rates -X PUT \
    -H "Content-Type: application/json" \
    -H "Authorization: Bearer hi_bob" \
    -d "{ \"ETH\" : 1, \"XRP\": 1 }" \
    &>/dev/null
```

### 8. Sending a Payment

<!--!
printf "\nChecking balances...\n"

printf "\nAlice's balance on Alice's node: "
curl \
-H "Authorization: Bearer hi_alice" \
http://localhost:7770/accounts/alice/balance

printf "\nBob's balance on Alice's node: "
curl \
-H "Authorization: Bearer hi_alice" \
http://localhost:7770/accounts/bob/balance

printf "\nAlice's balance on Bob's node: "
curl \
-H "Authorization: Bearer hi_bob" \
http://localhost:8770/accounts/alice/balance

printf "\nCharlie's balance on Bob's node: "
curl \
-H "Authorization: Bearer hi_bob" \
http://localhost:8770/accounts/charlie/balance

printf "\nBob's balance on Charlie's node: "
curl \
-H "Authorization: Bearer hi_charlie" \
http://localhost:9770/accounts/bob/balance

printf "\nCharlie's balance on Charlie's node: "
curl \
-H "Authorization: Bearer hi_charlie" \
http://localhost:9770/accounts/charlie/balance

printf "\n\n"
-->

The following script sends a payment from Alice to Charlie through Bob.

<!--! printf "Sending payment of 500 from Alice to Charlie through Bob\n" -->

```bash
curl \
    -H "Authorization: Bearer alice:alice_password" \
    -H "Content-Type: application/json" \
    -d "{\"receiver\":\"http://interledger-rs-node_c:7770/spsp/charlie\",\"source_amount\":500}" \
    http://localhost:7770/pay
```

<!--! printf "\n" -->

### 8. Check Balances

You may see unsettled balances before the settlement engines exactly work. Wait a few seconds and try later.

```bash #
printf "\nAlice's balance on Alice's node: "
curl \
-H "Authorization: Bearer hi_alice" \
http://localhost:7770/accounts/alice/balance

printf "\nBob's balance on Alice's node: "
curl \
-H "Authorization: Bearer hi_alice" \
http://localhost:7770/accounts/bob/balance

printf "\nAlice's balance on Bob's node: "
curl \
-H "Authorization: Bearer hi_bob" \
http://localhost:8770/accounts/alice/balance

printf "\nCharlie's balance on Bob's node: "
curl \
-H "Authorization: Bearer hi_bob" \
http://localhost:8770/accounts/charlie/balance

printf "\nBob's balance on Charlie's node: "
curl \
-H "Authorization: Bearer hi_charlie" \
http://localhost:9770/accounts/bob/balance

printf "\nCharlie's balance on Charlie's node: "
curl \
-H "Authorization: Bearer hi_charlie" \
http://localhost:9770/accounts/charlie/balance
```

<!--!
INCOMING_NOT_SETTLED=0
printf "\nChecking balances...\n"

printf "\nAlice's balance on Alice's node: "
curl \
-H "Authorization: Bearer hi_alice" \
http://localhost:7770/accounts/alice/balance

printf "\nBob's balance on Alice's node: "
curl \
-H "Authorization: Bearer hi_alice" \
http://localhost:7770/accounts/bob/balance

printf "\nAlice's balance on Bob's node: "
AB_BALANCE=`curl \
-H "Authorization: Bearer hi_bob" \
http://localhost:8770/accounts/alice/balance 2>/dev/null`
EXPECTED_BALANCE='{"balance":"0"}'
if [[ $AB_BALANCE != $EXPECTED_BALANCE ]]; then
    INCOMING_NOT_SETTLED=1
    printf "\e[33m$AB_BALANCE\e[m"
else
    printf $AB_BALANCE
fi

printf "\nCharlie's balance on Bob's node: "
curl \
-H "Authorization: Bearer hi_bob" \
http://localhost:8770/accounts/charlie/balance

printf "\nBob's balance on Charlie's node: "
BC_BALANCE=`curl \
-H "Authorization: Bearer hi_charlie" \
http://localhost:9770/accounts/bob/balance 2>/dev/null`
EXPECTED_BALANCE='{"balance":"0"}'
if [[ $BC_BALANCE != $EXPECTED_BALANCE ]]; then
    INCOMING_NOT_SETTLED=1
    printf "\e[33m$BC_BALANCE\e[m"
else
    printf $BC_BALANCE
fi

printf "\nCharlie's balance on Charlie's node: "
curl \
-H "Authorization: Bearer hi_charlie" \
http://localhost:9770/accounts/charlie/balance

printf "\n"

if [ $INCOMING_NOT_SETTLED -eq 1 ]; then
    printf "\n\e[33mThis means the incoming settlement is not done yet. It will be done once the block is generated or a ledger is validated.\n"
    printf "Try the following commands later:\n\n"
    printf "\tcurl -H \"Authorization: Bearer hi_bob\" http://localhost:8770/accounts/alice/balance\n"
    printf "\tcurl -H \"Authorization: Bearer hi_charlie\" http://localhost:9770/accounts/bob/balance\e[m"
fi
-->

### 9. Clear Redis
If you want to repeat the procedure, you can clear Redis data as follows.

```bash #
docker exec redis_a redis-cli flushall
docker exec redis_b redis-cli flushall
docker exec redis_c redis-cli flushall
```

### 10. Kill All the Services
Finally, you can stop all the services as follows:

<!--! printf "\n\nStopping Interledger nodes\n" -->

```bash #
docker stop interledger-rs-node_a interledger-rs-node_b interledger-rs-node_c interledger-rs-se_a interledger-rs-se_b interledger-rs-se_c interledger-rs-se_d ganache redis
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
cat logs/node-charlie-settlement-engine-xrpl.log | grep "Got incoming XRP payment"
```

<!--!
printf "\nYou could try the following command to check if a block is generated.\nTo check, you'll need to install geth.\n\n"
printf "To check the last block:\n"
printf "\tgeth --exec \"eth.getTransaction(eth.getBlock(eth.blockNumber-1).transactions[0])\" attach http://localhost:8545 2>/dev/null\n\n"
printf "To check the current block:\n"
printf "\tgeth --exec \"eth.getTransaction(eth.getBlock(eth.blockNumber).transactions[0])\" attach http://localhost:8545 2>/dev/null\n\n"
printf "You could also try the following command to check if XRPL incoming payment is done.\n\n"
printf "\tcat logs/node-charlie-settlement-engine-xrpl.log | grep \"Got incoming XRP payment\"\n\n"
-->

## Troubleshooting

> Uh oh! You need to install Docker before running this example

You need to install Docker to run this example. See [Prerequisites](#prerequisites) section.

> docker: Error response from daemon: Conflict. The container name "/interledger-rs-node_a" is already in use by container "xxx".
> You have to remove (or rename) that container to be able to reuse that name.

You seem to have run the other example, try [0. Clean up Docker](#0-clean-up-docker) first.

## Conclusion

This example showed an SPSP payment sent between three Interledger.rs nodes that settled using on-ledger Ethereum and XRPL transactions.

More examples that enhance your integration with ILP are coming soon!
