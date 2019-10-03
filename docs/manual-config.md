# Manual Testnet Configuration

## Get Your Own Credentials

In order to connect to the testnet, you need to generate
credentials and get your account configured on a testnet node.

Currently, the [the Xpring testnet website](https://xpring.io/ilp-testnet-creds) can be used to generate credentials. The site automatically configures your account on Xpring's testnet node. You could use the credentials on that node to send and receive payments, or you can use that account's information to connect your local node to the testnet (more on that later)

Just click the **"Generate XRP Credentials"** or **"Generate ETH Credentials"** button, and you'll get your account set on the node. Keep the credentials so that you could later send payments or set up your own node.

The credentials should look like (username and token would be different ones):

|key|value|
|---|---|
|username|user_qjex4cfk|
|token|us5w0zt1br4ft|
|ilp_over_http_url|https://rs3.xpring.dev/ilp|
|ilp_over_btp_url|btp+wss://rs3.xpring.dev/ilp/btp|
|payment_pointer|$rs3.xpring.dev/accounts/user_qjex4cfk/spsp|
|asset_code|XRP|
|asset_scale|9|

You can also obtain the above credentials by making a GET request to `https://xpring.io/api/accounts/xrp` or `https://xpring.io/api/accounts/eth`


## Send and Receive Payments
If you want to just try out the Interledger testnet, you could try as follows.

```bash
curl \
    -X POST \
    -H "Authorization: Bearer ${username}:${token}" \
    -H "Content-Type: application/json" \
    -d '{"receiver":"$their-payment-pointer.example","source_amount":500}' \
    https://rs3.xpring.dev/accounts/${Username}/payments
```

Note that you should replace **$their-payment-pointer.example** with one you want to send payments to, and **source_amount** with how much money you want to pay. If you are sending to an account that is configured with a different asset, the returned value may be different from the one you sent, based on any exchange rate calculations.

If someone sends you payments, you could confirm your balance increase as follows.

```bash
curl \
    -H "Authorization: Bearer ${username}:${token}" \
    https://rs3.xpring.dev/accounts/${Username}/balance
```

Note: Currently there is no way to check who paid you. This is a feature we intend to support in the future, and can be done by having the sender of the payment include additional metadata in the STREAM packets.



### Receive Payments

Other people could send you payments using the following SPSP address.

```bash
https://<YOUR_NAME>.localtunnel.me/accounts/<YOUR_NAME>/spsp
```

Then you can confirm your balance as follows:

```bash
docker run interledgerrs/ilp-cli --node https://<YOUR_NAME>.localtunnel.me accounts balance <YOUR_NAME> --auth <YOUR_NAME>:<YOUR_NAME>
```

### Manual configuration

**Configuring an Interledger Node is a complicated process. We highly advise you
use the provided docker image which has the Node, Engines and Store packaged
already. Only try doing the next steps if you want to get an in-depth view of
what is happening under the hood**

#### Requirements

Before you try to connect to the testnet, you need to install the following dependencies.

- localtunnel
- Redis
- Rust
- [settlement-xrp](https://github.com/interledgerjs/settlement-xrp) (to settle with XPRL)

1. **localtunnel**

    `localtunnel` exposes your local node to the internet. The others can make connections to your node, which means the Xpring's node would be able to connect to your node.

    ```bash
    npm install -g localtunnel
    ```

1. **Redis**

    The Interledger.rs nodes and settlement engines currently use [Redis](https://redis.io/) to store their data (SQL database support coming soon!). Nodes and settlement engines can use different Redis instances.

    - Compile and install from the source code
    - [Download the source code here](https://redis.io/download)
    - Install using package managers
    - Ubuntu: run `sudo apt-get install redis-server`
    - macOS: If you use Homebrew, run `brew install redis`

    Make sure your Redis is empty. You could run `redis-cli flushall` to clear all the data.

1. **Rust**
    Because Interledger.rs is written in the Rust language, you need the Rust environment. Refer to the [Getting started](https://www.rust-lang.org/learn/get-started) page or just `curl https://sh.rustup.rs -sSf | sh` and follow the instructions.

1. **settlement-xrp**

    Interledger.rs and settlement engines written in other languages are fully interoperable. Here, we'll use the [XRP Ledger Settlement Engine](https://github.com/interledgerjs/settlement-xrp/), which is written in TypeScript. We'll need `node` and `npm` to install and run the settlement engine. If you don't have it already, refer to [Install Node.js](#install-nodejs).

    Install the settlement engine as follows:

    ```bash
    npm install -g ilp-settlement-xrp
    ```

    (This makes the `ilp-settlement-xrp` command available to your PATH.)

### Running a node and peering

If you want to set up your environment manually, you have to:

- Spin up a settlement engine
- Configure and spin up a node
- Set up a localtunnel (in case you don't have your own global address)
- Insert accounts

#### Spin up a Settlement Engine

We currently have settlement engine support for XRP and Ethereum on-ledger Layer-1 transactions (payment channels support is in our pipeline). Set up one depending which asset you chose for your credentials.

To spin up a XRP settlement engine:

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

Note that we have to spin up a Redis which the settlement engine is going to connect to. It automatically get secrets from the XRP faucet, so you don't need to specify. If you want to use your own credentials, you have to provide the environment variable [`XRP_SECRET`](https://github.com/interledgerjs/settlement-xrp/blob/master/src/run.ts#L6). A faucet is available at the [XRPL website](https://xrpl.org/xrp-testnet-faucet.html).

To spin up a Ethereum settlement engine,

```bash
mkdir -p logs
redis-server --port 6380 &> logs/redis_se.log &

cargo run --all-features --bin interledger-settlement-engines -- ethereum-ledger \
--private_key "${SE_ETH_SECRET}" \
--chain_id 4 \
--confirmations 0 \
--poll_frequency 15000 \
--ethereum_url "${SE_ETH_URL}" \
--connector_url http://127.0.0.1:7771 \
--redis_url redis://127.0.0.1:6380/ \
--settlement_api_bind_address 127.0.0.1:3000 \
&> logs/settlement-engine-eth.log &
```

Note that you have to specify 2 parameters here, `SE_ETH_SECRET` and `SE_ETH_URL`. Because the Xpring node is connected to the Rinkeby testnet of Ethereum, we also need to connect to Rinkeby.

- `SE_ETH_SECRET`
    - The private key to be used. An Ethereum private key is a 32-byte hex string. You can generate one via [Ethereum Generate Wallet](https://github.com/vkobel/ethereum-generate-wallet).
- `SE_ETH_URL`
    - You can either sync an Ethereum node, in which case it will be `http://localhost:8545` or use [Infura](https://infura.io/), in which case the endpoint will look like `rinkeby.infura.io/v3/<API_KEY>`
- Funding your account
    - You can use the [Rinkeby Faucet](https://faucet.rinkeby.io). You will need to make a social media post (Twitter, Facebook, etc.) with your Ethereum address in it. You should then paste the link in the text box, click "Give me Ether" and select the "18.75 ethers / 3 days" option.

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

You have to replace `<secret_seed>` with your secret seed which is used to encrypt information. You could generate one with `openssl rand -hex 32`. `<admin_auth_token>` should be replaced with your admin token, which would be used when you make API calls to the admin-only endpoints of the node.

Then try:

```bash
redis-server --port 6379 &> logs/redis.log &
cargo run --all-features --bin ilp-node -- config.json &> logs/node.log &
```

Now you have your own node running locally🎉

#### Set up a `localtunnel`

In most cases, you will not have a global address for your node. In that case, you could utilize [`localtunnel`](http://localtunnel.me) to make the other nodes connect to your node without any global addresses. Try:

```bash
lt -p 7770 -s <YOUR_NAME> &>logs/localtunnel.log &
```

You can replace `YOUR_NAME` with any name of your choice. Your node will then be reachable via `<YOUR_NAME>.localtunnel.me`.

#### Insert Accounts

Now that our node is running and exposed to the internet, let's insert accounts. We will use the `ilp-cli` binary to insert the following:

1. Default settlement engines endpoints
    ```
    cargo run --bin ilp-cli settlement-engines set-all \
        --auth=<admin_auth_token> \
        --pair XRP http://localhost:3001 \
        --pair ETH http://localhost:3002
    ```

1. The main account that we use for sending and receiving payments.
    ```
    cargo run --bin ilp-cli accounts create <YOUR_NAME> \
        --auth=<admin_auth_token> \
        --asset-code=<ASSET_CODE> \
        --asset-scale=<ASSET_SCALE>
        --ilp-over-http-incoming-token=<YOUR_PASSWORD>
    ```

1. Connect to the Xpring Testnet node so we're connected to the rest of the testnet
    ```
    cargo run --bin ilp-cli testnet setup <ASSET_CODE> \
         --auth=<admin_auth_token> \
         --return-testnet-credential
    ```
    The above command will return the username and password, in the form of `USERNAME:PASSWORD` which we use to modify our account's settings the testnet node.

1. (Optional) Configure the testnet node to talk to us over HTTP instead of BTP (HTTP is more reliable, BTP is more efficient due to WebSockets)
   1. Configure the testnet account on our node to authenticate with ILP over HTTP
       ```
        cargo run --bin ilp-cli accounts update-settings xpring_<ASSET_CODE> \
                --auth=<admin_auth_token> \
                --ilp-over-http-incoming-token=xpring_password \
                --settle-threshold=1000 \
                --settle-to=0
        ```

   1.  Configure ILP over HTTP for our account on the testnet node
        ```
        cargo run --bin ilp-cli --node=https://rs3.xpring.dev accounts update-settings $USERNAME
                --auth=$TESTNET_AUTH \
                --ilp-over-http-outgoing-token=xpring_<ASSET_CODE>:xpring_password \
                --ilp-over-http-url=https://<YOUR_NAME>.localtunnel.me/ilp \
                --settle-threshold=1000 \
                --settle-to=0
        ```

    Notes:
        - `$USERNAME` is the first part of the returned value from the previous step.
        - `$TESTNET_AUTH` is the full `USERNAME:PASSWORD` string from the previous step


The peering process with the testnet is done! Congratulations!

You should now be able to send and receive payments through the testnet node.
