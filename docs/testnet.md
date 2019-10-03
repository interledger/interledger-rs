# Connecting to the Testnet

## Get Started

This tutorial shows you how to use a [Docker](https://www.docker.com/products/docker-desktop) image to easily connect to the testnet with a reproducible environment and use the [CLI](https://hub.docker.com/r/interledgerrs/ilp-cli) to interact with your node.

(Testnet uses fake or "play" money, which enables us to simulate the experience of running a real node without risking real funds!)

The Docker image bundles an Interledger node and settlement engines for XRP and ETH. It also automatically:

- Runs a new Interledger node
- Creates an admin account
- Creates a [Localtunnel](https://localtunnel.github.io/www/) address to publicly expose your Node
- Connects and peers with the [Xpring testnet node](https://xpring.io/ilp-testnet-creds) to send and receive payments with other nodes

You can obtain the images by executing the following commands, which download the latest images from Docker Hub:

```bash
# Get the image for the Node & settlement engines
docker pull interledgerrs/testnet-bundle
# Get the image for the CLI
docker pull interledgerrs/ilp-cli
```

Two nodes peered on Interledger must "settle" their Interledger packets using a [settlement engine](https://github.com/interledgerjs/settlement-core) for a particular ledger. The Docker image supports two different settlement mechanisms using cryptocurrencies: on-ledger XRP transfers and on-ledger Ethereum transfers. Choose your preferred currency!

## Run Node with XRP Settlement

To start your Interledger node and settle with the Xpring testnet node using on-ledger XRP transfers, run this command:

```bash
# Run the testnet bundle
docker run -it -e NAME=<YOUR_NAME> -e CURRENCY=XRP interledgerrs/testnet-bundle
```

In several seconds, your node should startup, generate a prefunded XRP testnet account, and be ready to send payments!

## Run Node with ETH Settlement

To settle with the Xpring testnet node using on-ledger ETH transfers, we need to generate a new Ethereum account and fund it from a faucet.

First, generate a new Ethereum private key (a 32-byte hex string) using [Vanity-Eth](https://vanity-eth.tk) by scrolling to the bottom and clicking "Generate." Then, click to reveal the private key. Make sure to record both the Ethereum private key and Ethereum address!

Second, you need to fund the Ethereum account on the Rinkeby testnet using [this faucet](https://faucet.rinkeby.io). Follow the instructions, which involve sending a tweet or creating a Facebook post with your Ethereum address, and providing a link to the post. Then, click "Give Me Ether" and select the "18.75 ethers / 3 days" option. The faucet should send you a payment on the Rinkeby testnet to fund your new wallet!

Lastly, to start the Interledger node with the Ethereum settlement engine:

```bash
# Run the testnet bundle
docker run -it -e NAME=<YOUR_NAME> -e CURRENCY=ETH -e ETH_SECRET_KEY=<YOUR_PRIVATE_KEY> interledgerrs/testnet-bundle
```

## Get Your Node's Public Address

The Docker image automatically exposes your Interledger node to the public internet using Localtunnel. This enables other nodes to send payments to you and to access the API. To find this URL, search for the "your url is" line in the terminal (in most cases, this should be `<YOUR_NAME>.localtunnel.me`).

## Send Payments

Now, you can send payments as follows:

```bash
docker run interledgerrs/ilp-cli --node https://<YOUR_NAME>.localtunnel.me pay <YOUR_NAME> \
     --auth <YOUR_NAME>:<YOUR_NAME> \
     --to <RECIPIENT_NODE_ADDRESS> \
     --amount <AMOUNT_TO_SEND>
```

This command:

1. Uses the payment pointer to connect to the recipient node and exchange details and the ILP address to setup the payment (using [SPSP](https://interledger.org/rfcs/0009-simple-payment-setup-protocol/))
1. Creates a [STREAM connection](https://interledger.org/rfcs/0029-stream/) to the recipient, which negotiates with the recipient to ensure as much money as possible is delivered
1. Sends many small ILP packets routed from your node, through Xpring's testnet node, to the recipient node
1. When the entire payment is complete, the delivered amount will be printed in your console!

(**Note**: to specify the amount, you must use _base units_. Both XRP and ETH accounts are configured to use amounts that are denomianted in 9 decimal places. So, to send the equivalent of 1 XRP or 1 ETH, the amount would be `1000000000`; to send 1 gwei, which is a very small amount of ETH, the amount would be `1`).

## Receive Payments

To receive payments, simply provide the URL of your node to a friend or node operator (or run another node yourself)! Then, they can use the `pay` command in the CLI, explained above, to send payments to you!

## Advanced Configuration

Want to learn how to setup the Node manually without using Docker? Checkout the [manual testnet configuration](./manual-config.md) tutorial!
