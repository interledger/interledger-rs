#!/bin/bash

if [ ! -z "$1"  ]
then
    curl localhost:7770/pay \
        -d "{ \"receiver\" : \"http://localhost:8770\", \"source_amount\": $1  }" \
        -H "Authorization: Bearer in_alice" -H "Content-Type: application/json"
fi

echo "Alice:Alice $(redis-cli hmget accounts:0 balance prepaid_amount)"
echo "Alice:Bob $(redis-cli hmget accounts:1 balance prepaid_amount)"

echo "----"

echo "Bob:Bob $(redis-cli -p 6380 hmget accounts:0 balance prepaid_amount)"
echo "Bib:Alice $(redis-cli -p 6380 hmget accounts:1 balance prepaid_amount)"

echo "----"

# dump the settlement transactions
geth --exec "eth.getTransaction(eth.getBlock(eth.blockNumber-1).transactions[0])" attach http://localhost:8545
geth --exec "eth.getTransaction(eth.getBlock(eth.blockNumber).transactions[0])" attach http://localhost:8545
