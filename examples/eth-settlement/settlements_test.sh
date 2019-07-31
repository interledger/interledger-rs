killall interledger interledger-settlement-engines node

# start ganache
ganache-cli -m "abstract vacuum mammal awkward pudding scene penalty purchase dinner depart evoke puzzle" -i 1 &> /dev/null &

sleep 3 # wait for ganache to start

ROOT=$HOME/projects/xpring-contract
ILP_DIR=$ROOT/interledger-rs
ILP=$ILP_DIR/target/debug/interledger
ILP_ENGINE=$ILP_DIR/target/debug/interledger-settlement-engines
CONFIGS=$ILP_DIR/examples

LOGS=$CONFIGS/eth-settlement/settlement_test_logs
rm -rf $LOGS && mkdir -p $LOGS
echo "Initializing redis"
bash $ILP_DIR/examples/init.sh &

sleep 1

ALICE_ADDRESS="3cdb3d9e1b74692bb1e3bb5fc81938151ca64b02"
ALICE_KEY="380eb0f3d505f087e438eca80bc4df9a7faa24f868e69fc0440261a0fc0567dc"
BOB_ADDRESS="9b925641c5ef3fd86f63bff2da55a0deeafd1263"
BOB_KEY="cc96601bc52293b53c4736a12af9130abf347669b3813f9ec4cafdf6991b087e"


echo "Initializing Alice SE"
RUST_LOG=interledger=debug $ILP_ENGINE ethereum-ledger \
--key $ALICE_KEY \
--server_secret aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa \
--confirmations 0 \
--poll_frequency 1000 \
--ethereum_endpoint http://127.0.0.1:8545 \
--connector_url http://127.0.0.1:7771 \
--redis_uri redis://127.0.0.1:6379 \
--watch_incoming true \
--port 3000 &> $LOGS/engine_alice.log &

echo "Initializing Bob SE"
RUST_LOG=interledger=debug $ILP_ENGINE ethereum-ledger \
--key $BOB_KEY \
--server_secret bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb \
--confirmations 0 \
--poll_frequency 1000 \
--ethereum_endpoint http://127.0.0.1:8545 \
--connector_url http://127.0.0.1:8771 \
--redis_uri redis://127.0.0.1:6380 \
--watch_incoming true \
--port 3001 &> $LOGS/engine_bob.log &

sleep 1

echo "Initializing Alice Connector"
RUST_LOG="interledger=debug,interledger=trace" $ILP node --config $CONFIGS/alice.yaml &> $LOGS/ilp_alice.log &
echo "Initializing Bob Connector"
RUST_LOG="interledger=debug,interledger=trace" $ILP node --config $CONFIGS/bob.yaml &> $LOGS/ilp_bob.log &

sleep 2
read -p "Press [Enter] key to continue..."

# insert alice's account details on Alice's connector
printf "\nInitializing Alice's account on her connector\n"
curl http://localhost:7770/accounts -X POST \
    -d "ilp_address=example.alice&asset_code=ETH&asset_scale=18&max_packet_amount=10&http_endpoint=http://127.0.0.1:7770/ilp&http_incoming_token=in_alice&outgoing_token=out_alice&settle_to=-10" \
    -H "Authorization: Bearer hi_alice"

printf "\n---------------------------------------\n"

# insert Bob's account details on Alice's connector
echo "Initializing Bob's account on Alice's connector (this will not return until Bob adds Alice in his connector)"
curl http://localhost:7770/accounts -X POST \
    -d "ilp_address=example.bob&asset_code=ETH&asset_scale=18&max_packet_amount=10&settlement_engine_url=http://127.0.0.1:3000&settlement_engine_asset_scale=18&http_endpoint=http://127.0.0.1:8770/ilp&http_incoming_token=bob&http_outgoing_token=alice&settle_threshold=70&min_balance=-100&settle_to=10" \
    -H "Authorization: Bearer hi_alice" &

printf "\n---------------------------------------\n"
read -p "Press [Enter] key to continue..."

# insert bob's account on bob's conncetor
printf "\nInitializing Bob's account on his connector\n"
curl http://localhost:8770/accounts -X POST \
    -d "ilp_address=example.bob&asset_code=ETH&asset_scale=18&max_packet_amount=10&http_endpoint=http://127.0.0.1:7770/ilp&http_incoming_token=in_bob&outgoing_token=out_bob&settle_to=-10" \
    -H "Authorization: Bearer hi_bob"

printf "\n---------------------------------------\n"
read -p "Press [Enter] key to continue..."
 
# insert Alice's account details on Bob's connector
# when setting up an account with another party makes senes to give them some slack if they do not prefund
echo "Initializing Alice's account on Bob's connector"
curl http://localhost:8770/accounts -X POST \
     -d "ilp_address=example.alice&asset_code=ETH&asset_scale=18&max_packet_amount=10&settlement_engine_url=http://127.0.0.1:3001&settlement_engine_asset_scale=18&http_endpoint=http://127.0.0.1:7770/ilp&http_incoming_token=alice&http_outgoing_token=bob&settle_threshold=70&min_balance=-100&settle_to=-10" \
     -H "Authorization: Bearer hi_bob" &

sleep 2
printf "\n---------------------------------------\n"
read -p "Press [Enter] key to continue..."

# Their settlement engines should be configured automatically 
echo "Alice Connector Store:"
redis-cli -p 6379 hgetall "accounts:1"
echo "Alice Engine Store:"
redis-cli -p 6379 hgetall "eth:ledger:settlement:1"

printf "\n---------------------------------------\n"

echo "Bob Connector Store:"
redis-cli -p 6380 hgetall "accounts:1"
echo "Bob Engine Store:"
redis-cli -p 6380 hgetall "eth:ledger:settlement:1"