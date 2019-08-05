# Must provide XRP_ENGINE_ROOT environment variable which is the path where
# https://github.com/interledgerjs/settlement-xrp is.
killall interledger interledger-settlement-engines node

ILP_DIR=$ILP_ROOT
ILP=$ILP_DIR/target/debug/interledger
E2E_TEST_DIR=$ILP_DIR/examples/e2e_tests/

LOGS=$E2E_TEST_DIR/xrp_ledger/settlement_test_logs
rm -rf $LOGS && mkdir -p $LOGS

bash $E2E_TEST_DIR/init.sh
sleep 1

echo "Initializing Alice SE"
DEBUG="xrp-settlement-engine" \
LEDGER_ADDRESS="rGCUgMH4omQV1PUuYFoMAnA7esWFhE7ZEV" \
LEDGER_SECRET="sahVoeg97nuitefnzL9GHjp2Z6kpj" \
CONNECTOR_URL="http://localhost:7771" \
REDIS_PORT=6379 \
ENGINE_PORT=3000 \
    node $XRP_ENGINE_ROOT/build/run.js &> $LOGS/xrp_engine_alice.log &

echo "Initializing Bob SE"
DEBUG="xrp-settlement-engine" \
LEDGER_ADDRESS="r3GDnYaYCk2XKzEDNYj59yMqDZ7zGih94K" \
LEDGER_SECRET="ssnYUDNeNQrNij2EVJG6dDw258jA6" \
CONNECTOR_URL="http://localhost:8771" \
REDIS_PORT=6380 \
ENGINE_PORT=3001 \
    node $XRP_ENGINE_ROOT/build/run.js &> $LOGS/xrp_engine_bob.log &

sleep 1

echo "Initializing Alice Connector"
RUST_LOG="interledger=debug,interledger=trace" $ILP node --config $E2E_TEST_DIR/alice.yaml &> $LOGS/ilp_alice.log &
echo "Initializing Bob Connector"
RUST_LOG="interledger=debug,interledger=trace" $ILP node --config $E2E_TEST_DIR/bob.yaml &> $LOGS/ilp_bob.log &

sleep 2
# read -p "Press [Enter] key to continue..."

# insert alice's account details on Alice's connector
printf "\nInitializing Alice's account on her connector\n"
curl http://localhost:7770/accounts -X POST \
    -d "ilp_address=example.alice&asset_code=XRP&asset_scale=6&max_packet_amount=10&http_endpoint=http://127.0.0.1:7770/ilp&http_incoming_token=in_alice&outgoing_token=out_alice&settle_to=-10" \
    -H "Authorization: Bearer hi_alice"

printf "\n---------------------------------------\n"

# insert Bob's account details on Alice's connector
echo "Initializing Bob's account on Alice's connector (this will not return until Bob adds Alice in his connector)"
curl http://localhost:7770/accounts -X POST \
    -d "ilp_address=example.bob&asset_code=XRP&asset_scale=6&max_packet_amount=10&settlement_engine_url=http://127.0.0.1:3000&http_endpoint=http://127.0.0.1:8770/ilp&http_incoming_token=bob&http_outgoing_token=alice&settle_threshold=70&min_balance=-100&settle_to=10" \
    -H "Authorization: Bearer hi_alice" &

printf "\n---------------------------------------\n"
# read -p "Press [Enter] key to continue..."

# insert bob's account on bob's conncetor
printf "\nInitializing Bob's account on his connector\n"
curl http://localhost:8770/accounts -X POST \
    -d "ilp_address=example.bob&asset_code=XRP&asset_scale=6&max_packet_amount=10&http_endpoint=http://127.0.0.1:7770/ilp&http_incoming_token=in_bob&outgoing_token=out_bob&settle_to=-10" \
    -H "Authorization: Bearer hi_bob"

printf "\n---------------------------------------\n"
# read -p "Press [Enter] key to continue..."

# insert Alice's account details on Bob's connector
# when setting up an account with another party makes senes to give them some slack if they do not prefund
echo "Initializing Alice's account on Bob's connector"
curl http://localhost:8770/accounts -X POST \
     -d "ilp_address=example.alice&asset_code=XRP&asset_scale=6&max_packet_amount=10&settlement_engine_url=http://127.0.0.1:3001&http_endpoint=http://127.0.0.1:7770/ilp&http_incoming_token=alice&http_outgoing_token=bob&settle_threshold=70&min_balance=-100&settle_to=-10" \
     -H "Authorization: Bearer hi_bob" &

sleep 2
printf "\n---------------------------------------\n"
# read -p "Press [Enter] key to continue..."

# Their settlement engines should be configured automatically 
echo "Alice Connector Store:"
redis-cli -p 6379 hgetall "accounts:1"
echo "Alice Engine Store:"
redis-cli -p 6379 get "xrp:accounts:1"

printf "\n---------------------------------------\n"
sleep 3

echo "Bob Connector Store:"
redis-cli -p 6380 hgetall "accounts:1"
echo "Bob Engine Store:"
redis-cli -p 6380 get "xrp:accounts:1" 
