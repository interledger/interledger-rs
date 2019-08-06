# Guide to E2E Testing Interledger settlement with 3 nodes utilizing ETH and XRP

First, you need an Ethereum network. We'll use
[ganache-cli](https://github.com/trufflesuite/ganache-cli) which deploys a local
Ethereum testnet at `localhost:8545`. You're free to use any other
testnet/mainnet you want. To install `ganache-cli`, run 
`npm install -g ganache-cli`. 

Secondly, you also need to download the XRP Engine from the upstream
[repository](https://github.com/interledgerjs/settlement-xrp/).

You also need to have `redis-server` and
`redis-cli` available in your PATH. In Ubuntu, you can obtain these by running
`sudo apt-get install redis-server`

Advanced: 
1. You can run this against the Rinkeby Testnet by running a node that
connects to Rinkeby (e.g. `geth --rinkeby --syncmode "light"`) or use a
third-party node provider such as [Infura](https://infura.io/). You must also
[create a wallet](https://www.myetherwallet.com/) and then obtain funds via
the [Rinkeby Faucet](https://faucet.rinkeby.io/).

1. Instead of using the `LEDGER_ADDRESS` and `LEDGER_SECRET` from the
examples below, you can generate your own XRPL credentials at the [official
faucet](https://xrpl.org/xrp-test-net-faucet.html).

1. Alice's
    1. Connector
    1. ETH Settlement Engine
    1. Redis Store
1. Bob's
    1. Connector
    1. ETH Settlement Engine
    1. XRP Settlement Engine
    1. Redis Store
1. Charlie's
    1. Connector
    1. XRP Settlement Engine
    1. Redis Store


We will need **10** terminal windows in total to follow this tutorial in depth.
You can run the provided `interoperable.sh` script instead to see how the full
process works.
ganache-cli -m "abstract vacuum mammal awkward pudding scene penalty purchase dinner depart evoke puzzle" -i 1 &> /dev/null &

## 1. Download the XRP Engine

1. git clone https://github.com/interledgerjs/settlement-xrp/
2. cd settlement-xrp && npm install && npm link

This makes the `ilp-settlement-xrp` command available to your path.

## 2. Launch Ganache

This will launch an Ethereum testnet with 10 prefunded accounts. The mnemonic is
used because we want to know the keys we'll use for Alice and Bob (otherwise
they are randomized)

```bash
ganache-cli -m "abstract vacuum mammal awkward pudding scene penalty purchase dinner depart evoke puzzle" -i 1
```

## 3. Configure Alice

1. In a new terminal, execute `redis-server --port 6379` to launch Redis for
   Alice.

1. Launch Alice's settlement engine in a new terminal by running:

```bash
RUST_LOG=interledger=debug $ILP settlement-engine ethereum-ledger \
--key "380eb0f3d505f087e438eca80bc4df9a7faa24f868e69fc0440261a0fc0567dc" \
--server_secret aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa \
--confirmations 0 \
--poll_frequency 1 \
--ethereum_endpoint http://127.0.0.1:8545 \
--connector_url http://127.0.0.1:7771 \
--redis_uri redis://127.0.0.1:6379 \
--asset_scale 6 \
--watch_incoming true \
--port 3000
```

1. Configure Alice's connector by putting the following data inside a config
   file, let's call that `alice.yml`. 

```yaml 
ilp_address: "example.alice"
secret_seed: "8852500887504328225458511465394229327394647958135038836332350604"
admin_auth_token: "hi_alice" 
redis_connection: "redis://127.0.0.1:6379"
settlement_address: "127.0.0.1:7771" 
http_address: "127.0.0.1:7770"
btp_address: "127.0.0.1:7768" 
default_spsp_account: "0" 
``` 

1. Launch Alice's connector in a new terminal by running:

```bash
RUST_LOG="interledger=debug,interledger=trace" $ILP node --config alice.yml
```

1. Insert Alice's account into her connector. 

```bash
curl http://localhost:7770/accounts -X POST \
    -d "ilp_address=example.alice&asset_code=ETH&asset_scale=6&max_packet_amount=10&http_endpoint=http://127.0.0.1:7770/ilp&http_incoming_token=in_alice&outgoing_token=out_alice&settle_to=-10" \
    -H "Authorization: Bearer hi_alice"
```

1. Insert Bob's account into Alice's connector. 

```bash
curl http://localhost:7770/accounts -X POST \
    -d "ilp_address=example.bob&asset_code=ETH&asset_scale=6&max_packet_amount=10&settlement_engine_url=http://127.0.0.1:3000&settlement_engine_asset_scale=6&http_endpoint=http://127.0.0.1:8770/ilp&http_incoming_token=bob&http_outgoing_token=alice&settle_threshold=70&min_balance=-100&settle_to=10&routing_relation=Peer&receive_routes=true&send_routes=true" \
    -H "Authorization: Bearer hi_alice" &
```

All set! Now Alice has her connector, settlement engine and redis store up and
running.

## 4. Configure Bob

1. In a new terminal, execute `redis-server --port 6380` to launch Redis for
   Bob.
1. Launch Bob's ETH settlement engine in a new terminal by running:

```bash
RUST_LOG=interledger=debug $ILP_ENGINE ethereum-ledger \
--key "cc96601bc52293b53c4736a12af9130abf347669b3813f9ec4cafdf6991b087e" \
--server_secret bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb \
--confirmations 0 \
--poll_frequency 1000 \
--ethereum_endpoint http://127.0.0.1:8545 \
--connector_url http://127.0.0.1:8771 \
--redis_uri redis://127.0.0.1:6380 \
--asset_scale 6 \
--watch_incoming true \
--port 3001
```

1. Launch Bob's XRP settlement engine in a new terminal by running:
```bash
DEBUG="xrp-settlement-engine" \
LEDGER_ADDRESS="r3GDnYaYCk2XKzEDNYj59yMqDZ7zGih94K" \
LEDGER_SECRET="ssnYUDNeNQrNij2EVJG6dDw258jA6" \
CONNECTOR_URL="http://localhost:8771" \
REDIS_PORT=6380 \
ENGINE_PORT=3002 \
    ilp-settlement-xrp
```

1. Configure Bob's connector by putting the following data inside a config
   file, let's call that `bob.yml`. 

```yaml 
ilp_address: "example.bob"
secret_seed: "1604966725982139900555208458637022875563691455429373719368053354"
admin_auth_token: "hi_bob"
redis_connection: "redis://127.0.0.1:6380"
settlement_address: "127.0.0.1:8771"
http_address: "127.0.0.1:8770"
btp_address: "127.0.0.1:8768"
default_spsp_account: "0"
``` 

1. Launch Bob's connector in a new terminal by running:

```bash
RUST_LOG="interledger=debug,interledger=trace" $ILP node --config bob.yml
```

1. Insert Alice's account on Bob's connector (ETH Peer relation)

```bash
curl http://localhost:8770/accounts -X POST \
     -d "ilp_address=example.alice&asset_code=ETH&asset_scale=6&max_packet_amount=10&settlement_engine_url=http://127.0.0.1:3001&settlement_engine_asset_scale=6&http_endpoint=http://127.0.0.1:7770/ilp&http_incoming_token=alice&http_outgoing_token=bob&settle_threshold=70&min_balance=-100&settle_to=-10&routing_relation=Peer&receive_routes=true&send_routes=true" \
     -H "Authorization: Bearer hi_bob"
```

1. Insert Charlie's account details on Bob's connector (XRP Child Relation)
```bash
curl http://localhost:8770/accounts -X POST \
    -d "ilp_address=example.bob.charlie&asset_code=XRP&asset_scale=6&max_packet_amount=10&settlement_engine_url=http://127.0.0.1:3002&settlement_engine_asset_scale=6&http_endpoint=http://127.0.0.1:9770/ilp&http_incoming_token=charlie&http_outgoing_token=bob&settle_threshold=70&min_balance=-100&settle_to=10&routing_relation=Child&receive_routes=true&send_routes=false" \
    -H "Authorization: Bearer hi_bob" &
```

## 5. Configure Charlie

1. In a new terminal, execute `redis-server --port 6381` to launch Redis for
   Charlie.
1. Launch Charlie's XRP settlement engine in a new terminal by running:

```bash
DEBUG="xrp-settlement-engine" \
LEDGER_ADDRESS="rGCUgMH4omQV1PUuYFoMAnA7esWFhE7ZEV" \
LEDGER_SECRET="sahVoeg97nuitefnzL9GHjp2Z6kpj" \
CONNECTOR_URL="http://localhost:9771" \
REDIS_PORT=6381 \
ENGINE_PORT=3003 \
    ilp-settlement-xrp &> $LOGS/xrp_engine_charlie.log &
```

1. Configure Charlie's connector by putting the following data inside a config
   file, let's call that `charlie.yml`.

```yaml 
ilp_address: "example.bob.charlie"
secret_seed: "1232362131122139900555208458637022875563691455429373719368053354"
admin_auth_token: "hi_charlie"
redis_connection: "redis://127.0.0.1:6381"
settlement_address: "127.0.0.1:9771"
http_address: "127.0.0.1:9770"
btp_address: "127.0.0.1:9768"
default_spsp_account: "0"
``` 

1. Launch Charlie's connector in a new terminal by running:

```bash
RUST_LOG="interledger=debug,interledger=trace" $ILP node --config charlie.yml
```

1. Insert Charlie's account details on Charlie's connector
```bash
curl http://localhost:9770/accounts -X POST \
    -d "ilp_address=example.bob.charlie&asset_code=XRP&asset_scale=6&max_packet_amount=10&http_endpoint=http://127.0.0.1:9770/ilp&http_incoming_token=in_charlie&outgoing_token=out_charlie&settle_to=-10" \
    -H "Authorization: Bearer hi_charlie"
```

1. Insert Bob's account details on Charlie's connector (XRP Parent relation)
```bash
curl http://localhost:9770/accounts -X POST \
    -d "ilp_address=example.bob&asset_code=XRP&asset_scale=6&max_packet_amount=10&settlement_engine_url=http://127.0.0.1:3003&settlement_engine_asset_scale=6&http_endpoint=http://127.0.0.1:8770/ilp&http_incoming_token=bob&http_outgoing_token=charlie&settle_threshold=70&min_balance=-100&settle_to=10&routing_relation=Parent&receive_routes=true&send_routes=false" \
    -H "Authorization: Bearer hi_charlie" &
```

## 6. Set the exchange rate between ETH and XRP on Bob since he's relaying

```bash
curl http://localhost:8770/rates -X PUT \
    -H "Content-Type: application/json" \
    -H "Authorization: Bearer hi_bob" \
    -d "{ \"ETH\" : 1, \"XRP\": 1 }"
```

## 7. Make some payments!

Alice pays Charlie through Bob:

```bash
curl localhost:7770/pay \
        -d "{ \"receiver\" : \"http://localhost:9770\", \"source_amount\": 70 }" \
        -H "Authorization: Bearer in_alice" -H "Content-Type: application/json"
```

If Alice pays Charlie again, settlement will trigger since they'll exceed the
`settle_threshold`. Alice will trigger a settlement to Bob which makes an
Ethereum ledger transaction, followed by Bob trigerring a settlement to Charlie
which makes an XRP ledger transaction.

Alice pays Charlie through Bob again:
```bash
curl localhost:8770/pay \
        -d "{ \"receiver\" : \"http://localhost:7770\", \"source_amount\": 1  }" \
        -H "Authorization: Bearer in_bob" -H "Content-Type: application/json"
```

You can inspect the balances of each account:
```bash
curl localhost:7770/accounts/0/balance -H "Authorization: Bearer hi_alice"
curl localhost:7770/accounts/1/balance -H "Authorization: Bearer hi_alice"
curl localhost:7770/accounts/0/balance -H "Authorization: Bearer hi_bob"
curl localhost:7770/accounts/1/balance -H "Authorization: Bearer hi_bob"
curl localhost:7770/accounts/0/balance -H "Authorization: Bearer hi_charlie"
curl localhost:7770/accounts/1/balance -H "Authorization: Bearer hi_charlie"
```
