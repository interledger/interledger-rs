#!/bin/bash

# who do we send to?
if [[ $1 == bob ]]
then
    PORT=7770
    RECEIVER=8770
    AUTH=in_alice
else
    PORT=8770
    RECEIVER=7770
    AUTH=in_bob
fi

if [ ! -z "$2"  ]
then
    set -x
    curl localhost:$PORT/pay \
        -d "{ \"receiver\" : \"http://localhost:$RECEIVER\", \"source_amount\": $2  }" \
        -H "Authorization: Bearer $AUTH" -H "Content-Type: application/json"
    set +x
fi

printf "\n----\n"

echo "Alice:Alice $(redis-cli hmget accounts:0 balance prepaid_amount)"
echo "Alice:Bob $(redis-cli hmget accounts:1 balance prepaid_amount)"

echo "----"

echo "Bob:Bob $(redis-cli -p 6380 hmget accounts:0 balance prepaid_amount)"
echo "Bib:Alice $(redis-cli -p 6380 hmget accounts:1 balance prepaid_amount)"

echo "----"

# dump the settlement transactions
geth --exec "eth.getTransaction(eth.getBlock(eth.blockNumber-1).transactions[0])" attach http://localhost:8545
geth --exec "eth.getTransaction(eth.getBlock(eth.blockNumber).transactions[0])" attach http://localhost:8545