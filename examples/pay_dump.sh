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

echo "Bob's balance on Alice's store"
curl localhost:7770/accounts/1/balance -H "Authorization: Bearer bob"

printf "\n----\n"

echo "Alice's balance on Bob's store"
curl localhost:8770/accounts/1/balance -H "Authorization: Bearer alice"

printf "\n----\n"

# dump the settlement transactions
geth --exec "eth.getTransaction(eth.getBlock(eth.blockNumber-1).transactions[0])" attach http://localhost:8545
geth --exec "eth.getTransaction(eth.getBlock(eth.blockNumber).transactions[0])" attach http://localhost:8545