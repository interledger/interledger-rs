# Connecting to the Testnet
> Try out the Interledger Protocol within a safe network.

This document explaians,

1. What is the testnet
1. How to have your own credentials of the testnet
1. How to connect a node to the testnet
1. How to make payments and receive payments on the node

Let's get started, play with Interledger!

## What Is the Testnet
In general, there are two types of networks.

1. The mainnet
1. The testnet

The mainnet is the production environment which is oriented to work for the real use. For instance, the mainnet of the Interledger Protocol would use the real money to transfer.

In contradiction to the mainnet, the testnet is to test functions of the network safely, which means for the Interledger Protocol the network doesn't deal with the real money. So the testnet is suitable for trying out what you built or even what you are building.

## Get Your Own Credentials
You need to get your own credentials to connect to the testnet. In other words, you need to get your account configured on a testnet node.

We have [the Xpring testnet site](https://test.xpring.io/ilp-testnet-creds) to provide credentials for you. The site automatically configures your account on the Xpring's testnet node. You could use the credentials to send payments from the node and receive payments to the node, or you could also connect your local node to the testnet.

Just push **"Generate XRP Credentials"** or **"Generate ETH Credentials"** button then you'll get your account set on the node. Keep the credentials so that you could later send payments or set up your own node.

The credentials should look like (Username and PassKey would be different ones):

|key|value|
|---|---|
|Username|user_qjex4cfk|
|PassKey|us5w0zt1br4ft|
|http_endpoint|https://rs3.xpring.dev/ilp|
|btp_endpoint|btp+wss://rs3.xpring.dev/ilp/btp|
|payment_pointer|$rs3.xpring.dev/accounts/user_qjex4cfk/spsp|
|asset_code|XRP|
|asset_scale|9|

## Send and Receive Payments
If you want to just try out the Interledger testnet, you could try as follows.

```bash
curl \
    -X POST \
    -H "Authorization: Bearer ${Username}:${PassKey}" \
    -H "Content-Type: application/json" \
    -d '{"receiver":"$their-payment-pointer.example","source_amount":500}' \
    https://rs3.xpring.dev/accounts/${Username}/payments
```

Note that you should replace **$their-payment-pointer.example** with one you want to send payments to, and **source_amount** with how much money you want to pay.

If someone sends you payments, you could confirm the balance as follows.

```bash
curl \
    -H "Authorization: Bearer ${Username}:${PassKey}" \
    https://rs3.xpring.dev/accounts/${Username}/balance
```

## Connect Your Local Node to the Testnet
There are two ways to connect your local node to the testnet.

- Use `connect-to-xpring.sh` to connect automatically
- Configure a node manually

### Prerequisites
Before you try to connect to the testnet, you need to install the following dependencies.

- localtunnel
- jq (to use `connect-to-xpring.sh`)
- Redis
- Rust
- [settlement-xrp](https://github.com/interledgerjs/settlement-xrp) (to settle with XPRL)

#### localtunnel

`localtunnel` exposes your local node to the internet. The others can make connections to your node, which means the Xpring's node would be able to connect to your node.

```bash
npm install -g localtunnel
```

#### jq

This is required for `connect-to-xpring.sh` because `connect-to-xpring.sh` manipulates JSON data.

- Compile and install from the source code
  - [Download the source code here](https://stedolan.github.io/jq/download/)
- Install using package managers
  - Ubuntu: run `sudo apt-get install jq`
  - macOS: If you use Homebrew, run `brew install jq`

#### Redis

The Interledger.rs nodes and settlement engines currently use [Redis](https://redis.io/) to store their data (SQL database support coming soon!). Nodes and settlement engines can use different Redis instances.

- Compile and install from the source code
  - [Download the source code here](https://redis.io/download)
- Install using package managers
  - Ubuntu: run `sudo apt-get install redis-server`
  - macOS: If you use Homebrew, run `brew install redis`

Make sure your Redis is empty. You could run `redis-cli flushall` to clear all the data.

#### Rust
Because Interledger.rs is written in the Rust language, you need the Rust environment. Refer to the [Getting started](https://www.rust-lang.org/learn/get-started) page or just `curl https://sh.rustup.rs -sSf | sh` and follow the instructions.

#### settlement-xrp

Interledger.rs and settlement engines written in other languages are fully interoperable. Here, we'll use the [XRP Ledger Settlement Engine](https://github.com/interledgerjs/settlement-xrp/), which is written in TypeScript. We'll need `node` and `npm` to install and run the settlement engine. If you don't have it already, refer to [Install Node.js](#install-nodejs).

Install the settlement engine as follows:

```bash
npm install -g ilp-settlement-xrp
```

(This makes the `ilp-settlement-xrp` command available to your PATH.)

### Configure Automatically with `connect-to-xpring.sh`

If you want to connect automatically, just try the following command in the `interledger-rs` directory.

```bash
./scripts/testnet/connect-to-xpring.sh -s -c
```

This configures `config.json` (`-c`) which is required to spin up a node and also spins up a settlement engine (`-s`). You just have to follow the instractions given by the script. It all automatically configures your local node to connect to the testnet through the Xpring's node.

### Configure a Node Manually

If you want to set up your environment manually, you have to:

- Spin up a settlement engine
- Configure and spin up a node
- Set up a localtunnel (in case you don't have your own global address)
- Insert accounts

#### Spin up a Settlement Engine

We have now two settlement engines: for XRP and Ethereum. Set up one depending which asset you chose for your credentials.

To spin up a XRP settlement engine,

```bash
mkdir -p logs
redis-server --port 6380 &> logs/redis_se.log &

DEBUG="settlement* ilp-settlement-xrp" \
CONNECTOR_URL="http://localhost:7771" \
REDIS_PORT=6380 \
ENGINE_PORT=3000 \
ilp-settlement-xrp \
&> logs/settlement-engine-xrpl.log &
```

Note that we have to spin up a Redis which the settlement engine is going to connect to. It automatically get secrets from the XRP faucet, so you don't need to specify.

To spin up a Ethereum settlement engine,

```bash
mkdir -p logs
redis-server --port 6380 &> logs/redis_se.log &

cargo run --all-features --bin interledger-settlement-engines -- ethereum-ledger \
--private_key "${SE_ETH_SECRET}" \
--chain_id 4 \
--confirmations 0 \
--poll_frequency 1000 \
--ethereum_url "${SE_ETH_URL}" \
--connector_url http://127.0.0.1:7771 \
--redis_url redis://127.0.0.1:6380/ \
--asset_scale 6 \
--settlement_api_bind_address 127.0.0.1:3000 \
&> logs/settlement-engine-eth.log &
```

Note that you have to specify 2 parameters here, `SE_ETH_SECRET` and `SE_ETH_URL`. Because the Xpring node is connected to the Rinkeby testnet of Ethereum, we also need to connect to Rinkeby.

- `SE_ETH_SECRET`
    - A private key of the Rinkeby testnet.
- `SE_ETH_URL`
    - An endpoint URL of the Rinkeby testnet.
    - You could use [Infura](https://infura.io/).

#### Configure and Spin up a Node

First, save your config file as `config.json` which contains:

```JSON
{
    "secret_seed": "<secret_seed>",
    "admin_auth_token": "<admin_auth_token>",
    "redis_url": "redis://127.0.0.1:6379/",
    "http_bind_address": "127.0.0.1:7770",
    "settlement_api_bind_address": "127.0.0.1:7771"
}
```

You have to replace `<secret_seed>` with your secret seed which is used to encrypt information. You could generate one with `openssl rand -hex 32`. Also `<admin_auth_token>` should be replaced with your admin token, which would be used when you make API calls to the node.

Then try:

```bash
redis-server --port 6379 &> logs/redis.log &
cargo run --all-features --bin ilp-node -- config.json &> logs/node.log &
```

Now you have your own node running locally🎉

#### Set up a `localtunnel`

In most cases, you might not have a global address for your node. In that case, you could utilize `localtunnel` to make the other nodes connect to your node without any global addresses. Try:

```
lt -p 7770 -s "${NODE_LT_SUBDOMAIN}" &>logs/localtunnel.log &
```

You have to replace `NODE_LT_SUBDOMAIN` with a subdomain of `localtunnel.me` which is finnaly going to be like `subdomain.localtunnel.me`.

#### Insert Accounts

Let's tackle a bit long one, accounts. Insert accounts in your node. You need to insert two accounts.

1. An account which represents the Xpring's node.
1. An account which represents your own address.

Save Xpring's account JSON as `xpring_account.json`.
```json
{
  "username": "xpring",
  "ilp_address": "test.xpring-dev.rs3",
  "asset_code": "${ASSET_CODE}",
  "asset_scale": 9,
  "ilp_over_btp_url": "btp+wss://rs3.xpring.dev/ilp/btp",
  "ilp_over_btp_incoming_token": "${XPRING_TOKEN}",
  "ilp_over_btp_outgoing_token": "${Username}:${PassKey}",
  "ilp_over_http_url": "https://rs3.xpring.dev/ilp",
  "ilp_over_http_incoming_token": "${XPRING_TOKEN}",
  "ilp_over_http_outgoing_token": "${Username}:${PassKey}",
  "settlement_engine_url": "http://localhost:3000",
  "settle_threshold": 10000,
  "settle_to": 0,
  "routing_relation": "Parent"
}
```

Make sure you replaced these:

- `ASSET_CODE` with the asset code you generated your credentials with.
- `XPRING_TOKEN` with some passphrases which the Xpring's node would use. You could generate one with `openssl rand -hex 32`.

Save your account JSON as `our_account.json`.
```json
{
  "username": "${Username}",
  "ilp_address": "test.xpring-dev.rs3.${Username}",
  "asset_code": "XRP",
  "asset_scale": 9,
  "ilp_over_http_incoming_token": "${PassKey}",
  "routing_relation": "NonRoutingAccount"
}
```

then

```bash
cat "xpring_account.json" | curl \
    -X POST \
    -H "Authorization: Bearer ${ADMIN_AUTH_TOKEN}" \
    -H "Content-Type: application/json" \
    -d @- \
    http://localhost:7770/accounts

cat "our_account.json" | curl \
    -X POST \
    -H "Authorization: Bearer ${ADMIN_AUTH_TOKEN}" \
    -H "Content-Type: application/json" \
    -d @- \
    http://localhost:7770/accounts
```

Note that you have to speficy `ADMIN_AUTH_TOKEN` which you set when you configured your node.

Next, you have to update your account on Xpring's node so that they would know where is our node. Save your `our_delegate_account.json` as follows:

```JSON
{
  "ilp_over_http_url": "https://${NODE_LT_SUBDOMAIN}.localtunnel.me/ilp",
  "ilp_over_http_incoming_token": "${PassKey}",
  "ilp_over_http_outgoing_token": "xpring:${XPRING_TOKEN}",
  "ilp_over_btp_url": "btp+wss://${NODE_LT_SUBDOMAIN}.localtunnel.me/ilp/btp",
  "ilp_over_btp_incoming_token": "${PassKey}",
  "ilp_over_btp_outgoing_token": "xpring:${XPRING_TOKEN}",
  "settle_threshold": 10000,
  "settle_to": 0,
}
```

You have to replace the following:

- `NODE_LT_SUBDOMAIN` with your `localtunnnel` subdomain.
- `PassKey` with your credential `PassKey`.
- `XPRING_TOKEN` with the one you specified for `xpring_account.json`.

Let's upload this JSON to the Xpring's node.

```
cat "our_delegate_account.json" | curl \
    -X PUT \
    -H "Authorization: Bearer ${Username}:${PassKey}" \
    -H "Content-Type: application/json" \
    -d @- \
    https://rs3.xpring.dev/accounts/${Username}/settings
```

Finally! You could set up the environment! Now you should be able to send and receive payments through the Xpring's node.


## Try Out Payments
### Send Payments

Now you can send payments as follows:

```
curl \
    -H "Authorization: Bearer ${Username}:${PassKey}" \
    -H "Content-Type: application/json" \
    -d '{"receiver":"$their-payment-pointer.example","source_amount": 500}' \
    http://localhost:7770/accounts/${Username}/payments
```

Note that you should replace **$their-payment-pointer.example** with one you want to send payments to, and **source_amount** with how much money you want to pay.

In this case, you are sending payment request to YOUR NODE. Your node resolves the payment pointer into an ILP address and then try to pay to the address. The ILP packets are routed through the Xpring's node because you are a child of the Xpring's node.

### Receive Payments

The other people could send you payments using the following SPSP address.

```
https://${NODE_LT_SUBDOMAIN}.localtunnel.me/accounts/${Username}/spsp
```

Then you can confirm your balance as follows:

```
curl \
    -H "Authorization: Bearer ${Username}:${PassKey}" \
    http://localhost:7770/accounts/${Username}/balance
```
