killall interledger node

# start ganache
ganache-cli -m "abstract vacuum mammal awkward pudding scene penalty purchase dinner depart evoke puzzle" -i 1 &> /dev/null &

sleep 3 # wait for ganache to start

ROOT=$HOME/projects/xpring-contract
ILP_DIR=$ROOT/interledger-rs
ILP=$ILP_DIR/target/debug/interledger


LOGS=`pwd`/settlement_test_logs
mkdir -p $LOGS

echo "Initializing redis"
bash $ILP_DIR/examples/init.sh &
redis-cli -p 6379 flushall
redis-cli -p 6380 flushall

sleep 1

ALICE_ADDRESS="3cdb3d9e1b74692bb1e3bb5fc81938151ca64b02"
ALICE_KEY="380eb0f3d505f087e438eca80bc4df9a7faa24f868e69fc0440261a0fc0567dc"
BOB_ADDRESS="9b925641c5ef3fd86f63bff2da55a0deeafd1263"
BOB_KEY="cc96601bc52293b53c4736a12af9130abf347669b3813f9ec4cafdf6991b087e"


echo "Initializing Alice SE"
RUST_LOG=interledger=debug $ILP settlement-engine ethereum-ledger \
--key $ALICE_KEY \
--server_secret aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa \
--confirmations 0 \
--poll_frequency 4 \
--ethereum_endpoint http://127.0.0.1:8545 \
--connector_url http://127.0.0.1:7771 \
--redis_uri redis://127.0.0.1:6379 \
--watch_incoming true \
--port 3000 &> $LOGS/engine_alice.log &

echo "Initializing Bob SE"
RUST_LOG=interledger=debug $ILP settlement-engine ethereum-ledger \
--key $BOB_KEY \
--server_secret bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb \
--confirmations 0 \
--poll_frequency 3 \
--ethereum_endpoint http://127.0.0.1:8545 \
--connector_url http://127.0.0.1:8771 \
--redis_uri redis://127.0.0.1:6380 \
--watch_incoming true \
--port 3001 &> $LOGS/engine_bob.log &

sleep 1

echo "Initializing Alice Connector"
RUST_LOG=interledger=debug $ILP node --config $ILP_DIR/configs/alice.yaml &> $LOGS/ilp_alice.log &
echo "Initializing Bob Connector"
RUST_LOG=interledger=debug $ILP node --config $ILP_DIR/configs/bob.yaml &> $LOGS/ilp_bob.log &

sleep 2

# insert alice's account details on Alice's connector
curl http://localhost:7770/accounts -X POST \
    -d "ilp_address=example.alice&asset_code=ETH&asset_scale=18&max_packet_amount=1&http_endpoint=http://127.0.0.1:7770/ilp&http_incoming_token=default&outgoing_token=default" \
    -H "Authorization: Bearer hi_alice"

# insert Bob's account details on Alice's connector
# alice should settle to bob whenever his balance is over 5
curl http://localhost:7770/accounts -X POST \
    -d "ilp_address=example.bob&asset_code=ETH&asset_scale=18&max_packet_amount=1&settlement_engine_url=http://127.0.0.1:3000&settlement_engine_asset_scale=18&settlement_engine_ilp_address=peer.settle.ethl&http_endpoint=http://127.0.0.1:8770/ilp&http_incoming_token=bob&http_outgoing_token=alice&settle_threshold=5&min_balance=-10" \
    -H "Authorization: Bearer hi_alice"

sleep 1

 curl http://localhost:8770/accounts -X POST \
     -d "ilp_address=example.bob&asset_code=ETH&asset_scale=18&max_packet_amount=1&http_endpoint=http://127.0.0.1:7770/ilp&http_incoming_token=default&outgoing_token=default" \
     -H "Authorization: Bearer hi_bob"
 
# insert Alice's account details on Bob's connector
# when setting up an account with another party makes senes to give them some slack if they do not prefund
curl http://localhost:8770/accounts -X POST \
     -d "ilp_address=example.alice&asset_code=ETH&asset_scale=18&max_packet_amount=1&settlement_engine_url=http://127.0.0.1:3001&settlement_engine_asset_scale=9&settlement_engine_ilp_address=peer.settle.ethl&http_endpoint=http://127.0.0.1:7770/ilp&http_incoming_token=alice&http_outgoing_token=bob&settle_threshold=5&min_balance=-10" \
     -H "Authorization: Bearer hi_bob"

sleep 1

# alice configures SE manually for bob's data, after knowing that bob has added her info to his store
curl http://localhost:3000/accounts/1 -X POST
# bob configures SE manually for alice's data, after knowing that alice has added his info to her store
curl http://localhost:3001/accounts/1 -X POST
# 
# # addresses must be exchanged
echo "Alice Store:"
redis-cli -p 6379 hgetall "settlement:ledger:eth:1"
echo "Bob Store:"
redis-cli -p 6380 hgetall "settlement:ledger:eth:1"

# Make an SPSP payment from Alice to Bob
echo 'Alice pays Bob'
curl localhost:7770/pay \
    -d '{ "receiver" : "http://localhost:8770", "source_amount": 1  }' \
    -H "Authorization: Bearer default" -H "Content-Type: application/json"

# TODO: Make this test keep running until a settlement actually happens. For that, amount_to_settle must be >0.
bob_balance=$(redis-cli -p 6379 hget "accounts:1" "balance") # bob must have positive balance for alice
echo "\nBob has $bob_balance from Alice's perspective."

alice_balance=$(redis-cli -p 6380 hget "accounts:1" "balance") # alice must have positive balance for alice
echo "\nAlice has $alice_balance from Bob's perspective"
