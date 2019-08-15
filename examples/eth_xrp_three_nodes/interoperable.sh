killall interledger interledger-settlement-engines node
# start ganache
ganache-cli -m "abstract vacuum mammal awkward pudding scene penalty purchase dinner depart evoke puzzle" -i 1 &> /dev/null &

# sleep 3 # wait for ganache to start

ROOT=$HOME/projects/xpring-contract
ILP_DIR=$ROOT/interledger-rs
ILP=$ILP_DIR/target/debug/interledger
ILP_ENGINE=$ILP_DIR/target/debug/interledger-settlement-engines
CONFIGS=$ILP_DIR/examples

LOGS=$CONFIGS/eth_xrp_three_nodes/settlement_test_logs
rm -rf $LOGS && mkdir -p $LOGS
echo "Initializing redis"
bash $ILP_DIR/examples/init.sh &
if lsof -Pi :6381 -sTCP:LISTEN -t >/dev/null ; then
    redis-cli -p 6381 shutdown
fi
redis-server --port 6381 --daemonize yes &> /dev/null
redis-cli -p 6381 flushall

sleep 1


echo "Initializing Alice ETH SE"
RUST_LOG=interledger=debug $ILP_ENGINE ethereum-ledger \
--key "380eb0f3d505f087e438eca80bc4df9a7faa24f868e69fc0440261a0fc0567dc" \
--server_secret aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa \
--confirmations 0 \
--poll_frequency 1000 \
--ethereum_endpoint http://127.0.0.1:8545 \
--connector_url http://127.0.0.1:7771 \
--redis_uri redis://127.0.0.1:6379 \
--asset_scale 6 \
--watch_incoming true \
--port 3000 &> $LOGS/engine_alice.log &

echo "Initializing Bob ETH SE"
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
--port 3001 &> $LOGS/engine_bob.log &

echo "Initializing Bob XRP SE"
DEBUG="xrp-settlement-engine" \
LEDGER_ADDRESS="r3GDnYaYCk2XKzEDNYj59yMqDZ7zGih94K" \
LEDGER_SECRET="ssnYUDNeNQrNij2EVJG6dDw258jA6" \
CONNECTOR_URL="http://localhost:8771" \
REDIS_PORT=6380 \
ENGINE_PORT=3002 \
    node $XRP_ENGINE_ROOT/build/run.js &> $LOGS/xrp_engine_bob.log &

echo "Initializing Charlie XRP SE"
DEBUG="xrp-settlement-engine" \
LEDGER_ADDRESS="rGCUgMH4omQV1PUuYFoMAnA7esWFhE7ZEV" \
LEDGER_SECRET="sahVoeg97nuitefnzL9GHjp2Z6kpj" \
CONNECTOR_URL="http://localhost:9771" \
REDIS_PORT=6381 \
ENGINE_PORT=3003 \
    node $XRP_ENGINE_ROOT/build/run.js &> $LOGS/xrp_engine_charlie.log &


sleep 1

echo "Initializing Alice Connector"
RUST_LOG="interledger=debug,interledger=trace" $ILP node --config $CONFIGS/alice.yaml &> $LOGS/ilp_alice.log &
echo "Initializing Bob Connector"
RUST_LOG="interledger=debug,interledger=trace" $ILP node --config $CONFIGS/bob.yaml &> $LOGS/ilp_bob.log &
echo "Initializing Charlie Connector"
RUST_LOG="interledger=debug,interledger=trace" $ILP node --config $CONFIGS/charlie.yaml &> $LOGS/ilp_charlie.log &

sleep 2
read -p "Press [Enter] key to continue..."

### Alice Config (Bob)

# insert alice's account details on Alice's connector
printf "\nInitializing Alice's account on her connector\n"
curl http://localhost:7770/accounts -X POST \
    -d "ilp_address=example.alice&asset_code=ETH&asset_scale=6&max_packet_amount=10&http_endpoint=http://127.0.0.1:7770/ilp&http_incoming_token=in_alice&outgoing_token=out_alice&settle_to=-10" \
    -H "Authorization: Bearer hi_alice"

printf "\n---------------------------------------\n"

# insert Bob's account details on Alice's connector (ETH Peer relation)
echo "Initializing Bob's account on Alice's connector (this will not return until Bob adds Alice in his connector)"
curl http://localhost:7770/accounts -X POST \
    -d "ilp_address=example.bob&asset_code=ETH&asset_scale=6&max_packet_amount=10&settlement_engine_url=http://127.0.0.1:3000&http_endpoint=http://127.0.0.1:8770/ilp&http_incoming_token=bob&http_outgoing_token=alice&settle_threshold=70&min_balance=-100&settle_to=10&routing_relation=Peer&receive_routes=true&send_routes=true" \
    -H "Authorization: Bearer hi_alice" &

printf "\n---------------------------------------\n"
read -p "Press [Enter] key to continue..."

### Bob config (Alice and Charlie)

# insert Alice's account details on Bob's connector (ETH Peer relation)
echo "Initializing Alice's account on Bob's connector (Alice has already added Bob as a peer so they should exchange SE addreses)"
curl http://localhost:8770/accounts -X POST \
     -d "ilp_address=example.alice&asset_code=ETH&asset_scale=6&max_packet_amount=10&settlement_engine_url=http://127.0.0.1:3001&http_endpoint=http://127.0.0.1:7770/ilp&http_incoming_token=alice&http_outgoing_token=bob&settle_threshold=70&min_balance=-100&settle_to=-10&routing_relation=Peer&receive_routes=true&send_routes=true" \
     -H "Authorization: Bearer hi_bob"

# insert Charlie's account details on Bob's connector (XRP Child Relation)
echo "Initializing Charlie's account on Bob's connector (this will not return until Charlie adds Bob in his connector)"
curl http://localhost:8770/accounts -X POST \
    -d "ilp_address=example.bob.charlie&asset_code=XRP&asset_scale=6&max_packet_amount=10&settlement_engine_url=http://127.0.0.1:3002&http_endpoint=http://127.0.0.1:9770/ilp&http_incoming_token=charlie&http_outgoing_token=bob&settle_threshold=70&min_balance=-100&settle_to=10&routing_relation=Child&receive_routes=true&send_routes=false" \
    -H "Authorization: Bearer hi_bob" &

sleep 2
printf "\n---------------------------------------\n"
read -p "Press [Enter] key to continue..."

# insert Charlie's account details on Charlie's connector
printf "\nInitializing Charlie's account on his connector\n"
curl http://localhost:9770/accounts -X POST \
    -d "ilp_address=example.bob.charlie&asset_code=XRP&asset_scale=6&max_packet_amount=10&http_endpoint=http://127.0.0.1:9770/ilp&http_incoming_token=in_charlie&outgoing_token=out_charlie&settle_to=-10" \
    -H "Authorization: Bearer hi_charlie"

printf "\n---------------------------------------\n"

# insert Bob's account details on Alice's connector (XRP Peer relation)
echo "Initializing Bob's account on Charlie's connector (this will not return until Bob adds Alice in his connector)"
curl http://localhost:9770/accounts -X POST \
    -d "ilp_address=example.bob&asset_code=XRP&asset_scale=6&max_packet_amount=10&settlement_engine_url=http://127.0.0.1:3003&http_endpoint=http://127.0.0.1:8770/ilp&http_incoming_token=bob&http_outgoing_token=charlie&settle_threshold=70&min_balance=-100&settle_to=10&routing_relation=Parent&receive_routes=true&send_routes=false" \
    -H "Authorization: Bearer hi_charlie" &

# Set the exchange rate between ETH and XRP

curl http://localhost:8770/rates -X PUT \
    -H "Content-Type: application/json" \
    -H "Authorization: Bearer hi_bob" \
    -d "{ \"ETH\" : 1, \"XRP\": 2 }"