# Guide to E2E Testing Interledger settlement with the XRP Ledger

You need to download the XRP Engine from the upstream
[repository](https://github.com/interledgerjs/settlement-xrp/). You also need to
have `redis-server` and `redis-cli` available in your PATH. In
Ubuntu, you can obtain these by running `sudo apt-get install redis-server`. We
use the XRP testnet for all settlement transactions. 

We will need **7** terminal windows in total to follow this tutorial in depth. You can run the
provided `settlement_test.sh` script instead to see how the full process works.

Advanced: Instead of using the `LEDGER_ADDRESS` and `LEDGER_SECRET` from the
examples below, you can generate your own XRPL credentials at the [official
faucet](https://xrpl.org/xrp-test-net-faucet.html).

1. Alice's
    1. Connector
    2. Settlement Engine
    3. Redis Store
1. Bob's
    1. Connector
    2. Settlement Engine
    3. Redis Store

For ease, you may want to set these environment variables to save you some
typing:
```
ILP_DIR=<path to interledger-rs>
ILP=$ILP_DIR/target/debug/interledger
```

## 1. Download the XRP Engine

1. git clone https://github.com/interledgerjs/settlement-xrp/
2. cd settlement-xrp && npm install && npm link

This makes the `ilp-settlement-xrp` command available to your path.

## 2. Configure Alice

1. In a new terminal, execute `redis-server --port 6379` to launch Redis for
   Alice.
1. Launch Alice's settlement engine in a new terminal by running:

```bash
DEBUG="xrp-settlement-engine" \
LEDGER_ADDRESS="rGCUgMH4omQV1PUuYFoMAnA7esWFhE7ZEV" \
LEDGER_SECRET="sahVoeg97nuitefnzL9GHjp2Z6kpj" \
CONNECTOR_URL="http://localhost:7771" \
REDIS_PORT=6379 \
ENGINE_PORT=3000 \
    ilp-settlement-xrp
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

The parameters are:
ILP Address: `example.alice`
Asset Code: `XRP`
Asset Scale: `6`
Max Packet Amount: `10`
Http Endpoint: `http://localhost:7770/ilp` (ilp over HTTP)
HTTP Incoming Token: `in_alice`
HTTP Outgoing Token: `out_alice`
Settle To: `10`

```bash
curl http://localhost:7770/accounts -X POST \
     -d "ilp_address=example.alice&asset_code=XRP&asset_scale=6&max_packet_amount=10&http_endpoint=http://127.0.0.1:7770/ilp&http_incoming_token=in_alice&outgoing_token=out_alice&settle_to=-10" \
     -H "Authorization: Bearer hi_alice"
```

All set! Now Alice has her connector, settlement engine and redis store up and
running.

## 3. Configure Bob

1. In a new terminal, execute `redis-server --port 6380` to launch Redis for
   Bob.
1. Launch Bob's settlement engine in a new terminal by running:

```bash
DEBUG="xrp-settlement-engine" \
LEDGER_ADDRESS="r3GDnYaYCk2XKzEDNYj59yMqDZ7zGih94K" \
LEDGER_SECRET="ssnYUDNeNQrNij2EVJG6dDw258jA6" \
CONNECTOR_URL="http://localhost:8771" \
REDIS_PORT=6380 \
ENGINE_PORT=3001 \
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

1. Insert Bob's account into his connector. 

The parameters are:
ILP Address: `example.bob`
Asset Code: `XRP`
Asset Scale: `6`
Max Packet Amount: `10`
Http Endpoint: `http://localhost:8770/ilp` (ilp over HTTP)
HTTP Incoming Token: `in_bob`
HTTP Outgoing Token: `out_bob`
Settle To: `10`

```bash
curl http://localhost:8770/accounts -X POST \
     -d "ilp_address=example.bob&asset_code=XRP&asset_scale=6&max_packet_amount=10&http_endpoint=http://127.0.0.1:8770/ilp&http_incoming_token=in_bob&outgoing_token=out_bob&settle_to=-10" \
     -H "Authorization: Bearer hi_bob"
```

Now we have both Alice and Bob up and running.

## 4. Peer each other.

### Insert Bob's account to Alice's connector
```bash
curl http://localhost:7770/accounts -X POST \
    -d "ilp_address=example.bob&asset_code=XRP&asset_scale=6&max_packet_amount=10&settlement_engine_url=http://127.0.0.1:3000&http_endpoint=http://127.0.0.1:8770/ilp&http_incoming_token=bob&http_outgoing_token=alice&settle_threshold=70&min_balance=-100&settle_to=10" \
    -H "Authorization: Bearer hi_alice
```

Notice how we use Alice's settlement engine endpoint while registering Bob. This
means that whenever Alice interacts with Bob's account, she'll use that
settlement engine.

__Parameter Explanation:__
- `settle_threshold`: Once an account's balance exceeds this value, it triggers
  a settlement
- `settle_to`: Once a settlement is triggered, the amount which should be paid
  is `current_balance - settle_to` (`settle_to` can also be negative,
  `current_balance` is the  account's current balance with the other party).
- `min_balance`: An account's balance cannot be less than this value (if
  positive this means that we require for the counterparty to pre-pay payments)

### Insert Alice's account to Bob's connector

```bash
curl http://localhost:8770/accounts -X POST \
     -d "ilp_address=example.alice&asset_code=XRP&asset_scale=6&max_packet_amount=10&settlement_engine_url=http://127.0.0.1:3001&http_endpoint=http://127.0.0.1:7770/ilp&http_incoming_token=alice&http_outgoing_token=bob&settle_threshold=70&min_balance=-100&settle_to=-10" \
     -H "Authorization: Bearer hi_bob"
```

## 5. Make some payments!

A `pay_dump.sh` script is provided which you can use to make [SPSP
payments](https://interledger.org/rfcs/0009-simple-payment-setup-protocol/)
between the two. After making an SPSP payment, it subsequently dumps the state
of the balance/prepaid_amount of each store, so that you can get a sense of what
is happening under the hood after each payment. You can also monitor your
connector and settlement engine's logs.

Alice pays Bob:
```bash
curl localhost:7770/pay \
        -d "{ \"receiver\" : \"http://localhost:8770\", \"source_amount\": 5  }" \
        -H "Authorization: Bearer in_alice" -H "Content-Type: application/json"
```

Bob pays Alice:
```bash
curl localhost:8770/pay \
        -d "{ \"receiver\" : \"http://localhost:7770\", \"source_amount\": 7  }" \
        -H "Authorization: Bearer in_bob" -H "Content-Type: application/json"
```

The net after this should be that Alice's store shows positive 2 balance on her
account and negative 2 for Bob. Bob's store should show the
opposite:

```bash
echo "Bob's balance on Alice's store"
curl localhost:7770/accounts/1/balance -H "Authorization: Bearer bob"

echo "Alice's balance on Bob's store"
curl localhost:8770/accounts/1/balance -H "Authorization: Bearer alice"
```

Done! Full E2E test between 2 users over SPSP utilizing the new settlement
architecture in an XRP Ledger settlement engine on testnet.
